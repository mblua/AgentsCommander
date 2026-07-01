use std::collections::HashMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::Manager;
use uuid::Uuid;

use crate::config::agent_config::AgentLocalConfig;
use crate::config::sessions_persistence::RaiseHandPersistOutcome;
use crate::config::settings::{AgentConfig, AppSettings, SettingsState};
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::session::{SessionCommunicationKind, SessionRepo};
use crate::session::session::{SessionInfo, SessionStatus};
use crate::{AppOutbox, MasterToken};

fn sender_name_for_session_cwd_with_root_flag(
    working_directory: &str,
    is_root_agent: bool,
) -> String {
    if is_root_agent {
        crate::config::root_agent::ROOT_AGENT_SENDER.to_string()
    } else {
        crate::config::teams::agent_fqn_from_path(working_directory)
    }
}

fn sender_name_for_session_cwd(working_directory: &str) -> String {
    let is_root_agent = crate::config::root_agent::is_root_agent_path(working_directory);
    sender_name_for_session_cwd_with_root_flag(working_directory, is_root_agent)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRefreshEventPayload {
    id: String,
    project_path: String,
    changed_path: Option<String>,
    changed_name: Option<String>,
    reason: String,
}

#[derive(Debug, Default)]
struct ProjectRefreshPollBatch {
    payloads: Vec<ProjectRefreshEventPayload>,
    processed_paths: Vec<PathBuf>,
}

fn canonical_project_refresh_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

fn project_refresh_priority(reason: &str) -> u8 {
    if reason == "projectRegistered" {
        0
    } else {
        1
    }
}

fn collect_project_refresh_requests(requests_dir: &Path) -> ProjectRefreshPollBatch {
    if !requests_dir.is_dir() {
        return ProjectRefreshPollBatch::default();
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(requests_dir) {
        Ok(rd) => rd
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect(),
        Err(e) => {
            log::warn!(
                "[project-refresh-requests] Failed to read {:?}: {}",
                requests_dir,
                e
            );
            return ProjectRefreshPollBatch::default();
        }
    };
    entries.sort();

    let mut batch = ProjectRefreshPollBatch::default();
    let mut selected_by_project: HashMap<String, (u8, ProjectRefreshEventPayload)> = HashMap::new();
    let mut project_order: Vec<String> = Vec::new();

    for path in entries {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let content = match read_text_bom_tolerant(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "[project-refresh-requests] Failed to read {:?}: {}",
                    path,
                    e
                );
                continue;
            }
        };

        let mut request: crate::cli::create_agent_matrix::ProjectRefreshRequest =
            match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "[project-refresh-requests] Failed to parse {:?}: {}",
                        path,
                        e
                    );
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };

        let canonical_project_path = canonical_project_refresh_path(&request.project_path);
        request.project_path = canonical_project_path.clone();
        batch.processed_paths.push(path);

        let priority = project_refresh_priority(&request.reason);
        let payload = ProjectRefreshEventPayload {
            id: request.id,
            project_path: request.project_path,
            changed_path: request.changed_path,
            changed_name: request.changed_name,
            reason: request.reason,
        };

        match selected_by_project.get(&canonical_project_path) {
            Some((existing_priority, _)) if *existing_priority <= priority => {}
            Some(_) => {
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
            None => {
                project_order.push(canonical_project_path.clone());
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
        }
    }

    for project_path in project_order {
        if let Some((_, payload)) = selected_by_project.remove(&project_path) {
            batch.payloads.push(payload);
        }
    }

    batch
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedWakeAgentCommand {
    shell: String,
    shell_args: Vec<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    source: String,
    raw_command: String,
}

fn normalize_agent_for_wake(
    agent: &AgentConfig,
    source: String,
) -> Result<ResolvedWakeAgentCommand, String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map_err(|e| {
        format!(
            "Invalid agent command from {} (agent id '{}', label '{}'): {}. command={:?}",
            source, agent.id, agent.label, e, agent.command
        )
    })?;

    Ok(ResolvedWakeAgentCommand {
        shell: normalized.shell,
        shell_args: normalized.shell_args,
        agent_id: Some(agent.id.clone()),
        agent_label: Some(agent.label.clone()),
        source,
        raw_command: agent.command.clone(),
    })
}

fn resolve_wake_agent_command_from_sources(
    agents: &[AgentConfig],
    preferred_agent: &str,
    destination_current_agent: Option<&str>,
    destination_last_agent: Option<&str>,
    destination_config_path: Option<&Path>,
    sender_agent: Option<&str>,
) -> Result<Option<ResolvedWakeAgentCommand>, String> {
    if preferred_agent != "auto" {
        if let Some(agent) = agents.iter().find(|a| a.id == preferred_agent) {
            return normalize_agent_for_wake(
                agent,
                format!("preferredAgent '{}'", preferred_agent),
            )
            .map(Some);
        }
        log::warn!(
            "[mailbox] wake: preferredAgent '{}' did not match a configured agent id; falling back to auto resolution",
            preferred_agent
        );
    }

    // The Selection UI assignment (currentCodingAgent) is an explicit user choice
    // and is what the "Entire Workgroup" coding-agent assignment writes to each
    // replica. Honor it before launch history (lastCodingAgent) so the assignment
    // propagates to freshly-spawned members that have never recorded a
    // lastCodingAgent, and so a new selection overrides a stale launch history.
    if let Some(agent_id) = destination_current_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            return normalize_agent_for_wake(agent, format!("currentCodingAgent '{}'", agent_id))
                .map(Some);
        }
        log::debug!(
            "[mailbox] wake: currentCodingAgent '{}' did not match a configured agent id; falling back",
            agent_id
        );
    }

    if let Some(agent_id) = destination_last_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            let source = match destination_config_path {
                Some(path) => format!("lastCodingAgent '{}' from {}", agent_id, path.display()),
                None => format!("lastCodingAgent '{}'", agent_id),
            };
            return normalize_agent_for_wake(agent, source).map(Some);
        }
        log::debug!(
            "[mailbox] wake: lastCodingAgent '{}' did not match a configured agent id; falling back",
            agent_id
        );
    }

    if let Some(agent_id) = sender_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            return normalize_agent_for_wake(agent, format!("senderAgent '{}'", agent_id))
                .map(Some);
        }
        log::debug!(
            "[mailbox] wake: senderAgent '{}' did not match a configured agent id; falling back",
            agent_id
        );
    }

    agents
        .first()
        .map(|agent| {
            normalize_agent_for_wake(agent, format!("first configured agent '{}'", agent.id))
        })
        .transpose()
}

fn validate_root_sender_route(
    to: &str,
    project_paths: &[String],
    is_master: bool,
    saw_session_token: bool,
    token_belongs_to_root_agent: bool,
) -> Result<(), &'static str> {
    if !is_master && (!saw_session_token || !token_belongs_to_root_agent) {
        return Err("Root Agent sender requires the live root session token");
    }

    if crate::config::teams::verified_wg_coordinator_target(to, project_paths).is_none() {
        return Err("Root Agent can only message verified WG coordinator replicas");
    }

    Ok(())
}

fn validate_coordinator_to_root_route(
    from: &str,
    project_paths: &[String],
) -> Result<(), &'static str> {
    if crate::config::teams::verified_wg_coordinator_target(from, project_paths).is_none() {
        return Err("Only verified WG coordinator replicas may message the Root Agent");
    }
    Ok(())
}

fn validate_root_sender_payload(msg: &OutboxMessage) -> Result<(), String> {
    let root_dir = crate::config::root_agent::root_agent_dir()?;
    validate_root_sender_payload_with_root_dir(msg, Path::new(&root_dir))
}

fn validate_root_sender_payload_with_root_dir(
    msg: &OutboxMessage,
    root_agent_dir: &Path,
) -> Result<(), String> {
    if msg.command.is_some() {
        return Err("Root Agent messages must use --send; remote commands are not allowed".into());
    }
    if msg.action.is_some() {
        return Err("Root Agent messages must use --send; action messages are not allowed".into());
    }

    let notification_path = crate::phone::messaging::parse_file_notification(&msg.body)
        .ok_or_else(|| "Root Agent messages must be canonical file notifications".to_string())?;
    let filename = crate::phone::messaging::notification_filename(notification_path)
        .ok_or_else(|| "Root Agent file notification must point to a Markdown file".to_string())?;
    crate::phone::messaging::validate_root_notification_filename(filename)
        .map_err(|e| format!("Root Agent file notification is invalid: {}", e))?;

    let notification_path = Path::new(notification_path);
    if !notification_path.is_absolute() {
        return Err("Root Agent file notification must use an absolute path".into());
    }
    let canon_file = std::fs::canonicalize(notification_path)
        .map_err(|e| format!("Root Agent file notification target is not readable: {}", e))?;
    if !canon_file
        .metadata()
        .map_err(|e| format!("Root Agent file notification target is not readable: {}", e))?
        .is_file()
    {
        return Err("Root Agent file notification target is not a regular file".into());
    }

    let root_messaging_dir = root_agent_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    let canon_root_messaging_dir = std::fs::canonicalize(&root_messaging_dir).map_err(|e| {
        format!(
            "Root Agent messaging directory is not readable at {}: {}",
            root_messaging_dir.display(),
            e
        )
    })?;
    if canon_file.parent() != Some(canon_root_messaging_dir.as_path()) {
        return Err(
            "Root Agent file notification must point inside ac-root-agent/messaging".into(),
        );
    }

    Ok(())
}

/// If `outbox_file` lives at
/// `<project_dir>/<workspace>/wg-<N>-*/__agent_*/<local-dir>/outbox/<file>.json`,
/// return `project_dir` as a UTF-8 `String`. Otherwise, `None`.
///
/// Mirrors the WG-replica walk-up in `cli::send::derive_root_project_dir` so
/// the mailbox can resolve qualified WG-peer FQNs against the same root-walk-up
/// source the sending CLI used (and the same source `list_peers::detect_wg_replica`
/// uses to report siblings as `reachable: true`). In-memory only — settings.json
/// is NOT mutated. See #228 D1-a.
///
/// The path layout is fixed by `MailboxPoller::poll` which constructs outbox
/// paths as `Path::new(<all_paths-entry>).join(agent_local_dir_name()).join("outbox")`.
/// We walk ancestors: `<file>.json` → `outbox` → `<local-dir>` → `<__agent_*>` →
/// `<wg-*>` → `<workspace>` → `<project_dir>`.
///
/// Uses `to_str()` (NOT `to_string_lossy()`) for parity with
/// `list_peers::detect_wg_replica`. The shared
/// `__agent_* -> wg-* -> <workspace> -> <project>` walk-up is delegated to
/// `config::workspace::wg_replica_layout_from_agent_dir` (single source with
/// `cli::send::derive_root_project_dir` and `list_peers::detect_wg_replica`,
/// see #726); only the outbox-specific `<file>.json -> outbox -> <local-dir>`
/// prefix to reach the `__agent_*` dir stays here.
fn derive_project_from_outbox_path(outbox_file: &Path) -> Result<Option<String>, String> {
    let Some(canon) = std::fs::canonicalize(outbox_file).ok() else {
        return Ok(None);
    };
    // canon = <project>/<workspace>/wg-*/__agent_*/<local-dir>/outbox/<file>.json
    let Some(outbox_dir) = canon.parent() else {
        return Ok(None);
    };
    if outbox_dir.file_name().and_then(|n| n.to_str()) != Some("outbox") {
        return Ok(None);
    }
    let Some(local_dir) = outbox_dir.parent() else {
        return Ok(None);
    };
    let Some(agent_dir) = local_dir.parent() else {
        return Ok(None);
    };
    match crate::config::workspace::wg_replica_layout_from_agent_dir(agent_dir)? {
        Some(layout) => Ok(layout.project_dir.to_str().map(|path| path.to_string())),
        None => Ok(None),
    }
}

/// Tracks delivery attempts for a single outbox message.
struct RetryState {
    attempt_count: u32,
    logged: bool,
}

const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";

/// #617 - sustained-idle window the deferred /clear waits for. `pub(crate)` so the
/// CLI prose and the response JSON single-source the value (no drift across the
/// gate, the response `settle_secs`, and the CLI's conditional wording).
pub(crate) const SELF_CLEAR_SETTLE_SECS: u64 = 30;
// The next three are consumed only by the `#[cfg(not(test))]` spawn in
// `handle_self_clear`; test builds drive `run_self_clear_after_sustained_idle`
// with explicit durations, so they are dead under `cfg(test)`.
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_SETTLE: std::time::Duration =
    std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// Safety cap so a never-idle session cannot leave a task polling forever.
/// Generous: any normal agent hits a 30s-idle window well within it.
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_MAX_DEFER: std::time::Duration = std::time::Duration::from_secs(3600);
/// #629 - grace delay after the Phase-2 handoff prompt is injected before `SELF-HANDOFF.md` is archived
/// to `self-clear/<ts>_SELF-HANDOFF.md`. 3 minutes is ample for the resumed agent to read the file;
/// archiving it then keeps a stale `SELF-HANDOFF.md` from false-triggering the NEXT cycle's existence gate (which
/// only checks the file's presence). In-memory only: a daemon restart inside the window leaves the file
/// (accepted, rare). Consumed only by the `cfg(not(test))`-spawned driver, so dead under `cfg(test)`.
#[cfg_attr(test, allow(dead_code))]
const SELF_HANDOFF_ARCHIVE_DELAY: std::time::Duration = std::time::Duration::from_secs(180);

/// #626 - the OutboxMessage `action` value for self-handoff-and-clear. Single-sourced so the CLI emit,
/// the early-dispatch match, and the response body cannot drift (a drift would make early dispatch
/// silently not fire and the command would be lost with no agent-visible error). `pub(crate)` so
/// `cli/self_clear.rs` reaches it as `crate::phone::mailbox::SELF_CLEAR_ACTION`.
pub(crate) const SELF_CLEAR_ACTION: &str = "self-handoff-and-clear";

pub(crate) const RAISE_HAND_ACTION: &str = "raise-hand";

/// #626 - stand-alone prompt injected in Phase 2 after the post-clear sustained-idle window. Must be a
/// SINGLE line (an embedded newline would submit early) and self-contained (the agent's context was just
/// wiped). `pub(crate)` so a test can assert it is non-empty, single-line, em-dash-free, and names the
/// file. The `\`-newline continuations collapse to one physical line with single spaces (no `\n`).
pub(crate) const SELF_CLEAR_HANDOFF_PROMPT: &str =
    "Your context was just cleared by the self-handoff-and-clear command. To resume, read the file \
     SELF-HANDOFF.md in your own agent root (your current working directory) and continue the work \
     described there. If SELF-HANDOFF.md is missing or empty, wait for new instructions instead of guessing.";

/// #668 - the OutboxMessage `action` value for self-handoff-and-switch.
pub(crate) const SELF_SWITCH_ACTION: &str = "self-handoff-and-switch";

/// #668 - prompt injected in Phase 2 after the switched session settles.
pub(crate) const SELF_SWITCH_HANDOFF_PROMPT: &str =
    "Your session was just switched by the self-handoff-and-switch command. To resume, read the file \
     SELF-HANDOFF.md in your own agent root (your current working directory) and continue the work \
     described there. If SELF-HANDOFF.md is missing or empty, wait for new instructions instead of guessing.";

pub(crate) const SELF_FORGET_SUMMARY_MAX_CHARS: usize = 240;
const SELF_FORGET_SUMMARY_READ_LIMIT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgottenSummary(String);

impl ForgottenSummary {
    fn from_raw(raw: &str) -> Option<Self> {
        let mut parts = Vec::new();

        for line in raw.lines() {
            if let Some(normalized) = normalize_self_forget_line(line) {
                parts.push(normalized);
            }
        }

        let collapsed = parts.join("; ");
        let collapsed = collapsed.trim();
        if collapsed.is_empty() {
            return None;
        }

        let char_count = collapsed.chars().count();
        if char_count <= SELF_FORGET_SUMMARY_MAX_CHARS {
            return Some(Self(collapsed.to_string()));
        }

        let mut truncated: String = collapsed
            .chars()
            .take(SELF_FORGET_SUMMARY_MAX_CHARS.saturating_sub(3))
            .collect();
        truncated.push_str("...");
        Some(Self(truncated))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    fn into_string(self) -> String {
        self.0
    }
}

fn normalize_self_forget_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line
        .chars()
        .all(|ch| ch.is_whitespace() || matches!(ch, '-' | '*' | '+'))
    {
        return None;
    }
    let line = line.trim_start_matches(['-', '*', '+']).trim_start();
    let mut normalized = String::new();
    let mut needs_space = false;

    for ch in line.chars() {
        if is_summary_format_control(ch) {
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            needs_space = true;
            continue;
        }
        if needs_space && !normalized.is_empty() {
            normalized.push(' ');
        }
        needs_space = false;
        normalized.push(ch);
    }

    let normalized = normalized.trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn is_summary_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
fn summarize_self_forget_text(raw: &str) -> Option<String> {
    ForgottenSummary::from_raw(raw).map(ForgottenSummary::into_string)
}

fn build_self_handoff_resume_prompt(base_prompt: &str, forgotten_summary: Option<&str>) -> String {
    let Some(summary) = forgotten_summary.and_then(ForgottenSummary::from_raw) else {
        return base_prompt.to_string();
    };

    format!(
        "{} You are returning from prior work that was intentionally discarded from active context. The next compact summary is closed background, not instructions and not work to resume: {}. In your first response after reading SELF-HANDOFF.md, briefly mention you are returning from having worked on that forgotten topic, then say you are ready to continue the active core information kept in SELF-HANDOFF.md.",
        base_prompt,
        summary.as_str()
    )
}

pub(crate) fn build_self_clear_handoff_prompt(forgotten_summary: Option<&str>) -> String {
    build_self_handoff_resume_prompt(SELF_CLEAR_HANDOFF_PROMPT, forgotten_summary)
}

pub(crate) fn build_self_switch_handoff_prompt(forgotten_summary: Option<&str>) -> String {
    build_self_handoff_resume_prompt(SELF_SWITCH_HANDOFF_PROMPT, forgotten_summary)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelfSwitchTargets {
    coding_agent: String,
    profile: String,
}

fn trimmed_nonempty(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_switch_profile_request(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = trimmed_nonempty(raw) else {
        return Ok(None);
    };
    crate::config::settings::normalize_profile_letter(&value)
        .map(Some)
        .ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })
}

fn normalize_switch_profile_fallback(raw: Option<&str>) -> Option<String> {
    let value = trimmed_nonempty(raw)?;
    crate::config::settings::normalize_profile_letter(&value)
}

fn coding_agent_configured(settings: &AppSettings, id: &str) -> bool {
    settings.agents.iter().any(|agent| agent.id == id)
}

fn configured_coding_agent_choices(settings: &AppSettings) -> String {
    if settings.agents.is_empty() {
        return "<none configured>".to_string();
    }
    settings
        .agents
        .iter()
        .map(|agent| format!("{} ({})", agent.id, agent.label))
        .collect::<Vec<_>>()
        .join(", ")
}

fn unknown_coding_agent_message(settings: &AppSettings, id: &str) -> String {
    format!(
        "self-handoff-and-switch: coding agent '{}' is not configured. Configured coding agents: {}",
        id,
        configured_coding_agent_choices(settings)
    )
}

fn resolve_switch_targets(
    settings: &AppSettings,
    cwd: &Path,
    req_agent: Option<&str>,
    req_profile: Option<&str>,
    session_agent: Option<&str>,
    session_effective_profile: Option<&str>,
    session_requested_profile: Option<&str>,
) -> Result<SelfSwitchTargets, String> {
    let coding_agent = if let Some(requested) = trimmed_nonempty(req_agent) {
        if !coding_agent_configured(settings, &requested) {
            return Err(unknown_coding_agent_message(settings, &requested));
        }
        Some(requested)
    } else if let Some(live) = trimmed_nonempty(session_agent) {
        Some(live)
    } else {
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(cwd)
            .and_then(|value| trimmed_nonempty(Some(value.as_str())))
            .filter(|value| coding_agent_configured(settings, value))
    };
    let coding_agent = coding_agent.ok_or_else(|| {
        format!(
            "self-handoff-and-switch: no target coding agent could be resolved. Pass --coding-agent <id>. Configured coding agents: {}",
            configured_coding_agent_choices(settings)
        )
    })?;

    let profile = normalize_switch_profile_request(req_profile)?
        .or_else(|| normalize_switch_profile_fallback(session_effective_profile))
        .or_else(|| normalize_switch_profile_fallback(session_requested_profile))
        .or_else(|| crate::config::coding_agent_profiles::read_replica_profile(cwd))
        .unwrap_or_else(|| "A".to_string());

    Ok(SelfSwitchTargets {
        coding_agent,
        profile,
    })
}

fn validate_self_switch_wg_replica(settings: &AppSettings, cwd: &Path) -> Result<PathBuf, String> {
    let name = cwd.file_name().and_then(|name| name.to_str()).ok_or_else(|| {
        format!(
            "self-handoff-and-switch is only supported from a WG replica (__agent_* under wg-*); '{}' has no valid final path component",
            cwd.display()
        )
    })?;
    if !name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a WG replica (__agent_* under wg-*); got '{}'",
            cwd.display()
        ));
    }
    let validated =
        crate::config::coding_agent_profiles::validate_profile_selection_agent_path(settings, cwd)
            .map_err(|e| {
                format!(
                    "self-handoff-and-switch is only supported from a configured WG replica: {}",
                    e
                )
            })?;
    let validated_name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !validated_name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a WG replica; validated target was '{}'",
            validated.launch_path.display()
        ));
    }
    Ok(validated.launch_path)
}

fn validate_self_switch_spawn(
    settings: &AppSettings,
    cwd: &Path,
    targets: &SelfSwitchTargets,
) -> Result<(), String> {
    if !coding_agent_configured(settings, &targets.coding_agent) {
        return Err(unknown_coding_agent_message(
            settings,
            &targets.coding_agent,
        ));
    }
    let profile =
        crate::config::settings::normalize_profile_letter(&targets.profile).ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })?;
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        &targets.coding_agent,
        Some(cwd),
        Some(&profile),
    )
    .map(|_| ())
    .map_err(|e| {
        format!(
            "self-handoff-and-switch: target coding agent '{}' profile '{}' is not launchable from '{}': {}",
            targets.coding_agent,
            profile,
            cwd.display(),
            e
        )
    })
}

/// §DR5 anti-spoof accept rule. Outbox-sender check passes when `msg_from`
/// equals `expected_from` exactly, OR when `msg_from` is unqualified (legacy)
/// AND its local part matches `expected_from`'s local part. Qualified-but-
/// wrong-project `msg_from` is always rejected.
pub(crate) fn anti_spoof_accept(msg_from: &str, expected_from: &str) -> bool {
    if msg_from == expected_from {
        return true;
    }
    let (_, exp_local) = crate::config::teams::split_project_prefix(expected_from);
    let (msg_proj, msg_local) = crate::config::teams::split_project_prefix(msg_from);
    msg_proj.is_none() && exp_local == msg_local
}

/// §AR2-norm step (1): upgrade a legacy-unqualified `msg_from` to
/// `expected_from` when expected_from is FQN. Returns true if the upgrade
/// happened. No-op when msg_from is already qualified, expected_from is
/// None, or expected_from itself is unqualified.
pub(crate) fn canonicalize_msg_from_in_place(
    msg_from: &mut String,
    expected_from: Option<&str>,
) -> bool {
    let Some(exp) = expected_from else {
        return false;
    };
    let (exp_proj, _) = crate::config::teams::split_project_prefix(exp);
    if exp_proj.is_none() {
        return false;
    }
    let (msg_proj, _) = crate::config::teams::split_project_prefix(msg_from);
    if msg_proj.is_some() {
        return false;
    }
    *msg_from = exp.to_string();
    true
}

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the agent
    /// is waiting for input or mid-turn. Bias toward delivery.
    Inject,
    /// Session is Exited — destroy it and fall through to spawn a fresh one.
    RespawnExited,
}

/// Pure decision given a session's status. Extracted so the decision table is
/// unit-testable without a tauri runtime. `deliver_wake` calls this and acts
/// on the result; any future restoration of a busy-gate would require editing
/// this fn (and its tests below), not a lone `if` inside `deliver_wake`.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}

/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback. Inverts
/// the positive-form `spawn_with_resume` flag so call sites read naturally.
///
/// Pinned via this helper to fence against a future refactor that "simplifies"
/// `!spawn_with_resume` to `spawn_with_resume` and silently regresses #82.
/// See plan §8.2 / round-2 R2.7 / round-3 R3.2.
pub(crate) fn wake_spawn_skip_auto_resume(spawn_with_resume: bool) -> bool {
    !spawn_with_resume
}

/// Decide whether a session is a viable candidate for the mailbox to attempt
/// delivery to. Pure function — unit-testable without a tauri runtime.
///
/// Rules:
/// - `Exited(_)` records are KEPT even with `has_pty == false` — the wake
///   contract documents the respawn path: "if Exited, respawn". The respawn
///   path does NOT need a live PTY (it calls `destroy_session_inner` then
///   `create_session_inner`).
/// - All other statuses (`Active`/`Running`/`Idle`) require `has_pty == true`.
///   A SessionManager record with one of these statuses but no PtyManager
///   entry is a desync phantom — the inject path is guaranteed to fail with
///   `AppError::SessionNotFound`, so the router must skip it (issue #223).
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision rather than silently routing
/// to the `has_pty` branch. (dev-rust R1.B7 / grinch G.M6.)
pub(crate) fn is_viable_wake_candidate(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => true,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// Decide whether a `SessionInfo` candidate is a viable Root Agent recipient
/// for delivery. Stricter than `is_viable_wake_candidate`: rejects
/// `SessionStatus::Exited(_)` outright.
///
/// The Root Agent is user-launched and never auto-respawned (#293 §3 contract).
/// `deliver_wake`'s deferred-destroy block fires for any candidate that hit
/// the `RespawnExited` arm, which would silently destroy the user's Root
/// Agent session record before the no-spawn guard fires. Returning `false`
/// for Exited keeps the record in place so the user sees it in the sidebar
/// exactly as they left it.
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision. Same pattern as
/// `is_viable_wake_candidate`.
pub(crate) fn is_viable_root_recipient(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => false,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// One tick of the #611 sustained-idle gate for a freshly spawned wake session.
///
/// A cold-starting agent (notably Claude) can hit a quiet window longer than the
/// idle threshold mid-startup and be marked `waiting_for_input` before its TUI
/// input/paste state is stable. Injecting then leaves the body in the input box
/// but the submit `\r` can be dropped, so the message is written yet never sent.
/// `wait_for_spawned_wake_idle` therefore requires idle to hold continuously for
/// a settle window before injecting; the existing double-Enter in `pty/inject.rs`
/// then submits reliably once the agent is stable.
///
/// Pure so the settle/reset policy is unit-testable without timers. Threads
/// `idle_since` (the instant the session was first seen continuously idle, or
/// `None` if it is currently busy / has not yet been observed idle) and returns
/// the next `idle_since` plus whether idle has now held for `settle`:
/// - busy (`waiting_for_input == false`) resets the clock to `None`: a late
///   startup render restarts the settle window.
/// - idle starts (or keeps) the clock; once `now - idle_since >= settle` the
///   caller may inject.
pub(crate) fn next_sustained_idle_state(
    waiting_for_input: bool,
    idle_since: Option<std::time::Instant>,
    now: std::time::Instant,
    settle: std::time::Duration,
) -> (Option<std::time::Instant>, bool) {
    if !waiting_for_input {
        return (None, false);
    }
    let since = idle_since.unwrap_or(now);
    // checked_duration_since guards against a non-monotonic `now` (parity with
    // idle_detector's threshold check); treat an unexpected backwards clock as
    // "not yet settled" rather than panicking.
    let settled = now
        .checked_duration_since(since)
        .map(|elapsed| elapsed >= settle)
        .unwrap_or(false);
    (Some(since), settled)
}

/// #626 - which leg of the self-handoff-and-clear gate we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfClearPhase {
    /// Waiting for sustained idle to inject `/clear`.
    Clear,
    /// `/clear` already injected; waiting for a FRESH sustained idle POST-clear to inject the
    /// stand-alone handoff prompt.
    Handoff,
}

/// #626 - gate state threaded by the driver across polls. Pure-function in/out, so the whole
/// two-stage policy (settle, reset, phase transition, per-phase cap, destroyed) is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SelfClearGateState {
    pub phase: SelfClearPhase,
    /// Instant the session was first seen continuously idle in the CURRENT phase, or None.
    pub idle_since: Option<std::time::Instant>,
    /// Start of the CURRENT phase, for the per-phase MAX_DEFER cap.
    pub phase_started: std::time::Instant,
}

impl SelfClearGateState {
    /// Initial state: Phase 1 (Clear), no idle observed yet, phase clock starts at `now`.
    pub(crate) fn new(now: std::time::Instant) -> Self {
        Self {
            phase: SelfClearPhase::Clear,
            idle_since: None,
            phase_started: now,
        }
    }
}

/// #626 - the action the driver must take after one decider step.
#[derive(Debug, PartialEq)]
pub(crate) enum SelfClearGateAction {
    /// Keep polling; the driver adopts the returned state and carries it forward.
    Wait,
    /// Phase 1 settle: inject `/clear`. The returned state is already advanced to Phase 2 with the
    /// idle clock reset, so the driver just injects and keeps looping.
    InjectClear,
    /// Phase 2 settle: inject the handoff prompt, then stop.
    InjectHandoff,
    /// Stop without injecting (session gone or per-phase cap reached); &str is the log reason.
    Abandon(&'static str),
}

/// #626 - pure two-stage gate step. Returns (next_state, action). `session_present == false` means
/// the session vanished between polls. The Clear->Handoff transition RESETS `idle_since` to None and
/// `phase_started` to `now`, so the Phase 2 window can NOT be satisfied by pre-clear idle. Same
/// "pure decision, thin timer loop" pattern as #617's `self_clear_gate_step` (replaced here), reusing
/// the #611 `next_sustained_idle_state` settle/reset core. Unit-testable WITHOUT timers, locks, or PTY.
pub(crate) fn self_clear_gate_advance(
    state: SelfClearGateState,
    session_present: bool,
    waiting_for_input: bool,
    now: std::time::Instant,
    settle: std::time::Duration,
    max_defer: std::time::Duration,
) -> (SelfClearGateState, SelfClearGateAction) {
    if !session_present {
        return (
            state,
            SelfClearGateAction::Abandon("session destroyed before sustained idle"),
        );
    }
    // Per-phase cap so each leg independently bounds a never-idle session.
    let phase_elapsed = now
        .checked_duration_since(state.phase_started)
        .unwrap_or_default();
    if phase_elapsed >= max_defer {
        let reason = match state.phase {
            SelfClearPhase::Clear => {
                "never reached sustained idle within MAX_DEFER cap (clear leg)"
            }
            SelfClearPhase::Handoff => {
                "never reached sustained idle within MAX_DEFER cap (handoff leg)"
            }
        };
        return (state, SelfClearGateAction::Abandon(reason));
    }
    let (next_idle_since, settled) =
        next_sustained_idle_state(waiting_for_input, state.idle_since, now, settle);
    if settled {
        match state.phase {
            SelfClearPhase::Clear => {
                // Advance to Handoff and RESET both clocks: pre-clear idle does not count.
                let next = SelfClearGateState {
                    phase: SelfClearPhase::Handoff,
                    idle_since: None,
                    phase_started: now,
                };
                (next, SelfClearGateAction::InjectClear)
            }
            SelfClearPhase::Handoff => (state, SelfClearGateAction::InjectHandoff),
        }
    } else {
        (
            SelfClearGateState {
                idle_since: next_idle_since,
                ..state
            },
            SelfClearGateAction::Wait,
        )
    }
}

/// #626/#629/#636 - archive `<root>/<stem>.md` to `<root>/self-clear/<timestamp>_<stem>.md`. No-op
/// (`Ok(None)`) if `<stem>.md` is absent. Creates the `self-clear/` subdir on demand. `timestamp` is
/// supplied by the caller so this is deterministic in tests. Returns the archived path on success.
/// `std::fs::rename` is atomic within the same filesystem (the agent root and its `self-clear/` child
/// share it). On Windows a source held open without FILE_SHARE_DELETE yields ERROR_SHARING_VIOLATION
/// (os error 32); the caller treats any `Err` as a non-fatal warn (no clobber, the source stays, next
/// cycle archives it). NO retries (a retry loop would block the caller). Consumers: SELF-FORGET.md at
/// queue time (#626) and SELF-HANDOFF.md after the post-handoff grace delay (#629).
fn archive_root_md(
    root: &std::path::Path,
    stem: &str,
    timestamp: &str,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let src = root.join(format!("{}.md", stem));
    if !src.is_file() {
        return Ok(None); // no-op when absent
    }
    let archive_dir = root.join("self-clear");
    let dst = archive_dir.join(format!("{}_{}.md", timestamp, stem));
    if dst.exists() {
        // Same-second collision (effectively impossible: archiving happens once per >=60s cycle).
        // Refuse to clobber an existing archive; leave the source in place.
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "archive target already exists",
        ));
    }
    std::fs::create_dir_all(&archive_dir)?;
    std::fs::rename(&src, &dst)?;
    Ok(Some(dst))
}

fn capture_self_forget_summary(root: &std::path::Path) -> Option<ForgottenSummary> {
    let path = root.join("SELF-FORGET.md");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: failed to open SELF-FORGET.md summary from {} (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let mut bytes = Vec::new();
    let mut bounded = file.take(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES);
    if let Err(e) = bounded.read_to_end(&mut bytes) {
        log::warn!(
            "[mailbox] self-handoff: failed to read SELF-FORGET.md summary from {} (non-fatal): {}",
            path.display(),
            e
        );
        return None;
    }

    let raw = match std::str::from_utf8(&bytes) {
        Ok(raw) => raw.to_string(),
        Err(e) if e.error_len().is_none() => {
            let valid_up_to = e.valid_up_to();
            std::str::from_utf8(&bytes[..valid_up_to])
                .unwrap_or("")
                .to_string()
        }
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: SELF-FORGET.md summary from {} is not valid UTF-8 (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    ForgottenSummary::from_raw(&raw)
}

/// Substring sniff for the `AppError::SessionNotFound` formatted message that
/// bubbles up through `pty::inject::inject_text_into_session` as a `String`.
///
/// Tight coupling: the error string format is `"Session not found: {uuid}"`
/// (see `errors.rs:5`). `inject_text_into_session` wraps it as `"PTY write
/// failed: Session not found: {uuid}"`. The substring `"Session not found:"`
/// covers both wrappings. Pinned by the
/// `err_is_pty_session_missing_matches_actual_apperror_format` test below.
///
/// If a future PR introduces typed-error plumbing through `inject_into_pty`,
/// replace this sniff with a typed match.
pub(crate) fn err_is_pty_session_missing(e: &str) -> bool {
    e.contains("Session not found:")
}

/// §224 D.3 — pure filter: session infos by exact-FQN match on
/// `working_directory`. Extracted from `find_all_sessions` so the predicate
/// can be unit-tested without a live `SessionManager` / `AppHandle`.
/// Defensive regression guard: no future change can accidentally re-introduce
/// a `was_active` / `waiting_for_input` / `status` gate without breaking
/// these tests.
pub(crate) fn filter_sessions_by_fqn<'a>(
    sessions: &'a [crate::session::session::SessionInfo],
    target: &str,
) -> Vec<&'a crate::session::session::SessionInfo> {
    sessions
        .iter()
        .filter(|s| session_cwd_matches_fqn(&s.working_directory, target))
        .collect()
}

fn session_cwd_matches_fqn(cwd: &str, target: &str) -> bool {
    crate::config::teams::agent_fqn_from_path(cwd) == target
}

fn resolve_wg_path_from_session_dirs(dirs: &[(Uuid, String)], agent_name: &str) -> Option<String> {
    let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
    let (wg_name, agent_short) = local.split_once('/')?;
    if !wg_name.starts_with("wg-") {
        return None;
    }

    let wg_marker = format!("/{}/", wg_name);
    for (_, cwd) in dirs {
        let normalized = cwd.replace('\\', "/");
        if let Some(wg_pos) = normalized.rfind(&wg_marker) {
            let wg_dir = &normalized[..wg_pos + 1 + wg_name.len()];
            let candidate = format!("{}/__agent_{}", wg_dir, agent_short);
            if !std::path::Path::new(&candidate).is_dir() {
                continue;
            }

            let candidate_fqn = crate::config::teams::agent_fqn_from_path(&candidate);
            if let Some(want) = target_project {
                let (cand_project, _) = crate::config::teams::split_project_prefix(&candidate_fqn);
                if cand_project != Some(want) {
                    continue;
                }
            }

            log::debug!(
                "[mailbox] wake: resolved WG agent path from sibling session: {}",
                candidate
            );
            return Some(
                std::path::PathBuf::from(&candidate)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    None
}

/// §224 A.2.5 — outcome of the daemon-restart race guard wait loop.
///
/// The production caller (`handle_close_session`) inlines the wait loop
/// because the natural probe closure captures `&self`, `app`, and `target`
/// across an `.await`, which is awkward to pass through this helper's
/// `FnMut() -> Future` shape without boxing. The helper is retained as the
/// canonical executable specification of the wait semantics for D.5a unit
/// tests — any future divergence between the inline loop and this helper is
/// a regression.
///
/// §224 G-IMPL-5 (NIT, accepted) — LOAD-BEARING COMMENT. The inlined wait
/// loop in `handle_close_session` has no direct unit test on Windows
/// (D.5b is `#[ignore]`'d pending the cross-process FS enumeration
/// investigation). Equivalence between the two implementations is
/// enforced ONLY by this comment + the helper's D.5a unit tests. Do NOT
/// change the inlined loop's semantics without updating this helper to
/// match, and vice versa.
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub(crate) enum RestoreWaitOutcome {
    /// Deadline elapsed with the restore flag still set; caller should report
    /// `status="restore_in_progress"` to the user.
    StillInProgress,
    /// Flag cleared with the probe still returning empty — there is no live
    /// session for this FQN. Caller falls through to the `no_match` path.
    NoMatch,
}

/// §224 A.2.5 — poll `probe` for a non-empty result, OR for `flag` to clear,
/// until `deadline`. Pure async helper extracted so the race-guard logic can
/// be unit-tested without a live `SessionManager` / `AppHandle`.
///
/// Returns:
///   * `Ok(non_empty_session_ids)` — a session appeared during the wait.
///   * `Err(StillInProgress)` — deadline elapsed, flag still set.
///   * `Err(NoMatch)` — flag cleared with empty result throughout (final
///     probe also empty).
#[allow(dead_code)]
pub(crate) async fn wait_for_restore_or_session<F, Fut>(
    flag: &std::sync::atomic::AtomicBool,
    mut probe: F,
    deadline: std::time::Instant,
    poll: std::time::Duration,
) -> Result<Vec<Uuid>, RestoreWaitOutcome>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<Uuid>>,
{
    use std::sync::atomic::Ordering;
    loop {
        if std::time::Instant::now() >= deadline {
            return if flag.load(Ordering::SeqCst) {
                Err(RestoreWaitOutcome::StillInProgress)
            } else {
                Err(RestoreWaitOutcome::NoMatch)
            };
        }
        tokio::time::sleep(poll).await;
        let ids = probe().await;
        if !ids.is_empty() {
            return Ok(ids);
        }
        if !flag.load(Ordering::SeqCst) {
            // Flag cleared. One final probe to catch a last-tick insertion
            // by the restore task before its guard dropped.
            let ids = probe().await;
            return if ids.is_empty() {
                Err(RestoreWaitOutcome::NoMatch)
            } else {
                Ok(ids)
            };
        }
    }
}

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
#[cfg(test)]
type MailboxAttachCalls = Arc<Mutex<Vec<(Uuid, Option<String>)>>>;

#[cfg(test)]
#[derive(Clone, Default)]
struct MailboxTestHooks {
    pty_presence: Arc<Mutex<HashMap<Uuid, bool>>>,
    inject_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    inject_calls: Arc<Mutex<Vec<Uuid>>>,
    destroy_calls: Arc<Mutex<Vec<Uuid>>>,
    spawn_calls: Arc<Mutex<Vec<MailboxSpawnCall>>>,
    attach_calls: MailboxAttachCalls,
    events: Arc<Mutex<Vec<MailboxTestEvent>>>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MailboxSpawnCall {
    to: String,
    session_name: String,
    cwd: String,
    shell: String,
    shell_args: Vec<String>,
    skip_auto_resume: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum MailboxTestEvent {
    Inject(Uuid),
    Destroy(Uuid),
    Spawn(MailboxSpawnCall),
    Attach {
        session_id: Uuid,
        bot_id: Option<String>,
    },
}

pub struct MailboxPoller {
    poll_interval: std::time::Duration,
    retry_tracker: HashMap<PathBuf, RetryState>,
    #[cfg(test)]
    test_hooks: Option<MailboxTestHooks>,
}

impl Default for MailboxPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl MailboxPoller {
    pub fn new() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            #[cfg(test)]
            test_hooks: None,
        }
    }

    #[cfg(test)]
    fn new_with_test_hooks(test_hooks: MailboxTestHooks) -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            test_hooks: Some(test_hooks),
        }
    }

    /// Start the poller as a background task.
    pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            // Initial poll without delay (matches original behavior)
            if let Err(e) = self.poll(&app).await {
                log::warn!("MailboxPoller error: {}", e);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => {
                        log::info!("[MailboxPoller] Shutdown signal received, stopping");
                        break;
                    }
                    _ = tokio::time::sleep(self.poll_interval) => {
                        if let Err(e) = self.poll(&app).await {
                            log::warn!("MailboxPoller error: {}", e);
                        }
                    }
                }
            }
        });
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    async fn poll(&mut self, app: &tauri::AppHandle) -> Result<(), String> {
        let settings = app.state::<SettingsState>();
        let repo_paths = {
            let cfg = settings.read().await;
            cfg.project_paths.clone()
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_working_dirs().await
        };

        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        // Include the instance-private app outbox
        let app_outbox = app.state::<AppOutbox>();
        let app_outbox_path = app_outbox.path().to_string();

        // Collect all outbox directories to scan
        let mut outbox_dirs: Vec<PathBuf> = all_paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .join(crate::config::agent_local_dir_name())
                    .join("outbox")
            })
            .collect();
        outbox_dirs.push(PathBuf::from(&app_outbox_path));

        for outbox_dir in &outbox_dirs {
            if !outbox_dir.is_dir() {
                continue;
            }

            let entries: Vec<PathBuf> = match std::fs::read_dir(outbox_dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                    .filter(|p| {
                        // Skip files in subdirectories (delivered/, rejected/)
                        p.parent() == Some(outbox_dir.as_path())
                    })
                    .collect(),
                Err(_) => continue,
            };

            let is_app_outbox = outbox_dir.as_path() == Path::new(&app_outbox_path);
            for path in entries {
                match self.process_message(app, &path, is_app_outbox).await {
                    Ok(()) => {
                        self.retry_tracker.remove(&path);
                    }
                    Err(e) => {
                        let is_permanent = e.contains(ERR_UNRESOLVABLE_AGENT);
                        let should_reject = is_permanent || {
                            let state =
                                self.retry_tracker
                                    .entry(path.clone())
                                    .or_insert(RetryState {
                                        attempt_count: 0,
                                        logged: false,
                                    });
                            state.attempt_count += 1;

                            if !state.logged {
                                log::warn!(
                                    "Failed to process outbox message {:?} (attempt {}): {}",
                                    path,
                                    state.attempt_count,
                                    e
                                );
                                state.logged = true;
                            } else {
                                log::debug!(
                                    "Retry {} for outbox message {:?}: {}",
                                    state.attempt_count,
                                    path,
                                    e
                                );
                            }

                            state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                        };

                        if should_reject {
                            let reason = if is_permanent {
                                e.clone()
                            } else {
                                let attempts = self
                                    .retry_tracker
                                    .get(&path)
                                    .map(|s| s.attempt_count)
                                    .unwrap_or(0);
                                format!(
                                    "Undeliverable after {} attempts. Last error: {}",
                                    attempts, e
                                )
                            };

                            // §130-stuck-file: on read failure (e.g. non-UTF-8 non-BOM file),
                            // fall back to `reject_raw_file` so the file is moved to `rejected/`
                            // instead of looping forever with `attempt_count >= MAX`.
                            let rejected = match read_text_bom_tolerant(&path) {
                                Ok(content) => {
                                    if let Ok(msg) = serde_json::from_str::<OutboxMessage>(&content)
                                    {
                                        self.reject_message(&path, &msg, &reason).await.is_ok()
                                    } else {
                                        Self::reject_raw_file(&path, &reason).is_ok()
                                    }
                                }
                                Err(_) => Self::reject_raw_file(&path, &reason).is_ok(),
                            };

                            if rejected {
                                self.retry_tracker.remove(&path);
                            } else {
                                log::error!(
                                    "Failed to reject outbox message {:?} — will retry",
                                    path
                                );
                            }
                        }
                    }
                }
            }
        }

        // Prune tracker entries for files that no longer exist
        self.retry_tracker.retain(|path, _| path.exists());

        // Poll project-refresh-requests directory from create-agent-matrix CLI.
        self.poll_project_refresh_requests(app).await;

        // Poll session-requests directory (from create-agent CLI)
        self.poll_session_requests(app).await;

        Ok(())
    }

    /// Process a single outbox message file.
    /// `is_app_outbox`: true if the message came from the instance-private outbox (master token path).
    async fn process_message<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let content = read_text_bom_tolerant(path)
            .map_err(|e| format!("Failed to read outbox file: {}", e))?;

        // `let mut msg`: §AR2-norm below mutates `msg.from` / `msg.to` in place
        // as the SINGLE POINT OF TRUTH for canonicalization. Downstream code
        // (routing, action dispatch, injection, archival) reads the canonical
        // form without re-mutation.
        let mut msg: OutboxMessage = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id,
            msg.from,
            msg.to,
            msg.mode
        );

        let outbox_project = match derive_project_from_outbox_path(path) {
            Ok(project) => project,
            Err(reason) => {
                return self.reject_message(path, &msg, &reason).await;
            }
        };

        // For repo outboxes (not app-outbox), validate that msg.from matches the outbox owner.
        // This prevents tokenless spoofing: a message in repo X's outbox must claim to be from repo X.
        //
        // §DR5 lenient fallback: if `msg.from` is unqualified (legacy) and its LOCAL
        // part matches `expected_from`'s local part, accept — §AR2-norm below then
        // upgrades `msg.from` to the canonical FQN. Cross-project qualified names
        // are rejected (different `project:` prefix).
        //
        // `expected_from` is hoisted (§DR2-3) so §AR2-norm can upgrade `msg.from`
        // even when this block has returned to outer scope.
        let mut expected_from: Option<String> = None;
        if !is_app_outbox {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let derived = sender_name_for_session_cwd(&repo_path.to_string_lossy());
                if !anti_spoof_accept(&msg.from, &derived) {
                    return self
                        .reject_message(
                            path,
                            &msg,
                            &format!(
                                "Outbox-sender mismatch: outbox belongs to '{}' but message claims '{}'",
                                derived, msg.from
                            ),
                        )
                        .await;
                }
                expected_from = Some(derived);
            }
        }

        // ── §AR2-norm — SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization ──
        //
        // Runs AFTER anti-spoof and BEFORE token validation / routing / action dispatch.
        // Every downstream read of msg.from / msg.to sees the canonical FQN (or bare
        // legacy form when canonicalization wasn't possible). Downstream code MUST
        // NOT re-mutate msg.from or msg.to.

        // (1) Upgrade a legacy-unqualified msg.from to the anti-spoof-derived
        // expected_from FQN (closes grinch §G5: resolve_repo_path(&msg.from)
        // for response-dir lookup now sees a canonical input).
        let original_from_for_log = msg.from.clone();
        if canonicalize_msg_from_in_place(&mut msg.from, expected_from.as_deref()) {
            log::debug!(
                "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
                original_from_for_log,
                msg.from
            );
        }

        // (2) Canonicalize msg.to via the shared resolver. Empty `to` is allowed for
        // action-dispatch paths (e.g. close-session may set an empty to); skip in
        // that case. Reject-on-ambiguity semantics match the CLI (Decision 2 rule 2c).
        //
        // §AR2-norm D1-a augmentation: when the outbox file lives under a WG-replica
        // layout (`<project>/<workspace>/wg-*/__agent_*/<local-dir>/outbox/<file>.json`),
        // include the derived `<project>` in the in-memory `paths` slice so qualified
        // WG-peer FQNs written by `cli::send` (which performs the symmetric walk-up
        // augmentation — see #228 Step 1) resolve here too. Without this, the daemon
        // re-rejects with `UnknownQualified` even though the CLI side succeeded.
        // settings.json is NOT mutated. In-memory, this-message only. See #228 D1-a.
        if !msg.to.is_empty() {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = outbox_project.as_ref() {
                let canon_root_project = std::fs::canonicalize(root_project).ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        std::fs::canonicalize(p).ok().as_ref() == Some(canon_target)
                    }
                    None => p == root_project,
                });
                if !already_present {
                    paths.push(root_project.clone());
                }
            }
            match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
                Ok(fqn) => {
                    if fqn != msg.to {
                        log::debug!("[mailbox] canonicalized msg.to '{}' → '{}'", msg.to, fqn);
                        msg.to = fqn;
                    }
                }
                Err(e) => {
                    return self
                        .reject_message(path, &msg, &format!("Unresolvable target: {}", e))
                        .await;
                }
            }
        }

        // Check if token is the master token or root token (bypasses anti-spoofing + team validation)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        let root_agent_claim = msg.from == crate::config::root_agent::ROOT_AGENT_SENDER;
        let root_agent_recipient = crate::config::root_agent::is_root_agent_target(&msg.to);
        let mut token_belongs_to_root_agent = false;
        let mut saw_session_token = false;

        if !is_master {
            // Validate session token if present (anti-spoofing)
            if let Some(ref token_str) = msg.token {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;

                if let Ok(token_uuid) = Uuid::parse_str(token_str) {
                    match mgr.find_by_token(token_uuid).await {
                        None => {
                            // Token is stale/invalid. Env-only credentials cannot be refreshed into
                            // an already-running child process, so reject instead of injecting a new token.
                            drop(mgr);
                            if let Some(session_id) = self.find_active_session(app, &msg.from).await
                            {
                                log::warn!(
                                    "[mailbox] Stale token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
                                    msg.from,
                                    session_id
                                );
                            }
                            return self
                                .reject_message(
                                    path,
                                    &msg,
                                    "Invalid session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
                                )
                                .await;
                        }
                        Some(session) => {
                            saw_session_token = true;
                            token_belongs_to_root_agent = session.is_root_agent
                                || crate::config::root_agent::is_root_agent_path(
                                    &session.working_directory,
                                );
                            // Anti-spoofing: verify msg.from matches the token's session CWD.
                            // Post-§AR2-norm, msg.from is canonical FQN (or legacy unqualified
                            // if expected_from was unavailable). Session-derived name uses the
                            // canonical helper; comparison is exact equality.
                            let session_name = sender_name_for_session_cwd_with_root_flag(
                                &session.working_directory,
                                token_belongs_to_root_agent,
                            );
                            if session_name != msg.from {
                                log::warn!(
                                    "[mailbox] Token-root mismatch: token session='{}' but from='{}'",
                                    session_name, msg.from
                                );
                                return self.reject_message(
                                    path,
                                    &msg,
                                    &format!(
                                        "Token-root mismatch: session is '{}' but message claims '{}'",
                                        session_name, msg.from
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                } else {
                    // Token is not a valid UUID. Env-only credentials cannot be refreshed
                    // into an already-running child process, so reject instead of injecting.
                    drop(mgr);
                    if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                        log::warn!(
                            "[mailbox] Malformed token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
                            msg.from,
                            session_id
                        );
                    }
                    return self
                        .reject_message(
                            path,
                            &msg,
                            "Malformed session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
                        )
                        .await;
                }
            }
        }

        // ── #617 self-clear: token-authorized self-operation ──
        // Dispatched BEFORE the team-routing chain below. self-clear clears the
        // CALLER'S OWN context; it is authorized solely by session-token ownership
        // (proved just above: find_by_token + from==session_name anti-spoof), not by
        // "can A reach B" team rules. Routing it through can_communicate would wrongly
        // reject valid callers in no shared team. close-session stays in the
        // post-routing action block because it IS a cross-agent operation.
        if msg.action.as_deref() == Some(SELF_CLEAR_ACTION) {
            return self.handle_self_clear(app, path, &msg, is_app_outbox).await;
        }
        if msg.action.as_deref() == Some(SELF_SWITCH_ACTION) {
            return self
                .handle_self_handoff_switch(app, path, &msg, is_app_outbox)
                .await;
        }
        if msg.action.as_deref() == Some(RAISE_HAND_ACTION) {
            return self.handle_raise_hand(app, path, &msg, is_app_outbox).await;
        }

        if root_agent_claim {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = outbox_project.as_ref() {
                let canon_root_project = std::fs::canonicalize(root_project).ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        std::fs::canonicalize(p).ok().as_ref() == Some(canon_target)
                    }
                    None => p == root_project,
                });
                if !already_present {
                    paths.push(root_project.clone());
                }
            }

            if let Err(reason) = validate_root_sender_route(
                &msg.to,
                &paths,
                is_master,
                saw_session_token,
                token_belongs_to_root_agent,
            ) {
                return self.reject_message(path, &msg, reason).await;
            }
            if let Err(reason) = validate_root_sender_payload(&msg) {
                return self.reject_message(path, &msg, &reason).await;
            }
            log::debug!(
                "[mailbox] Root Agent routing check passed: '{}' -> '{}'",
                msg.from,
                msg.to
            );
        } else if root_agent_recipient {
            // #293 — coordinator → root recipient.
            //
            // Build the same effective_project_paths slice the root-sender
            // branch uses (settings + the outbox file's WG-replica project
            // walk-up) so verified_wg_coordinator_target sees the project
            // where the outbox lives.
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = outbox_project.as_ref() {
                let canon_root_project = std::fs::canonicalize(root_project).ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        std::fs::canonicalize(p).ok().as_ref() == Some(canon_target)
                    }
                    None => p == root_project,
                });
                if !already_present {
                    paths.push(root_project.clone());
                }
            }

            // Master/root token does NOT bypass the verified-coordinator check
            // for root-recipient: the URI is meaningful only when paired with
            // a real coordinator identity, and the verified check is cheap.
            if let Err(reason) = validate_coordinator_to_root_route(&msg.from, &paths) {
                return self.reject_message(path, &msg, reason).await;
            }
            log::debug!(
                "[mailbox] Coordinator→Root routing check passed: '{}' -> '{}'",
                msg.from,
                msg.to
            );
        } else if is_master {
            log::debug!(
                "[mailbox] Master token used — bypassing team validation for {} -> {}",
                msg.from,
                msg.to
            );
        } else {
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return self
                    .reject_message(path, &msg, "Sender cannot reach destination")
                    .await;
            }
            log::debug!(
                "[mailbox] Routing check passed: '{}' -> '{}'",
                msg.from,
                msg.to
            );
        }

        // Action-based dispatch (close-session, etc.) — handled before mode-based delivery
        if let Some(ref action) = msg.action {
            match action.as_str() {
                "close-session" => {
                    // §224 G-IMPL-2 — thread `is_app_outbox` so the response-
                    // write path can skip the outbox-relative primary write
                    // when the message came from the app-outbox (master-token
                    // path). That derived path lands under
                    // <config_dir>/instances/<id>/responses/ which the CLI
                    // never polls, so writing there leaks orphan JSON files.
                    return self
                        .handle_close_session(app, path, &msg, is_app_outbox)
                        .await;
                }
                _ => {
                    return self
                        .reject_message(path, &msg, &format!("Unknown action '{}'", action))
                        .await;
                }
            }
        }

        // Deliver based on mode — all modes require immediate delivery or rejection
        let mode = if msg.mode.is_empty() {
            "wake"
        } else {
            msg.mode.as_str()
        };
        // Only `wake` is supported. Defensive check for malformed outbox files
        // that might arrive from external (root-token) write paths.
        if mode != "wake" {
            return self
                .reject_message(
                    path,
                    &msg,
                    &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
                )
                .await;
        }
        self.deliver_wake(app, &msg).await?;

        // Move to delivered/ with token stripped
        self.move_to_delivered(path, &msg).await
    }

    /// Deliver mode: wake — inject into the recipient's PTY for any non-Exited
    /// session; destroy and respawn if Exited; spawn persistent if none. Always
    /// delivers (no busy-gate — stdin buffer absorbs input while the agent is
    /// mid-turn).
    async fn deliver_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        // Whether the spawn-fallback should allow provider auto-resume.
        // Default false: cold wake — no SessionManager record at this CWD.
        // Promoted to true in two paths below: (a) RespawnExited deferred-
        // destroy block, (b) phantoms-only fall-through (every CWD match
        // was a desync phantom — preserve any on-disk transcript via
        // auto-resume). See issue #223 round-1 resolution.
        //
        // MUST NOT be re-derived after `destroy_session_inner` runs: post-
        // destroy, the candidate would vanish from list_sessions and the
        // value would silently flip, regressing the deferred-non-coord
        // wake by losing `--continue`. Set the flag inside the candidate
        // loop only. See plan §4.5.a / round-1 G7 / round-3 R3.2.
        let mut spawn_with_resume = false;
        let mut pending_exited_destroy: Option<Uuid> = None;
        let mut pending_exited_telegram_bot_id: Option<String> = None;
        // HIGH-1 (Step-7 review): symmetric AC5 protection. Tracks whether
        // every iter'd Inject candidate hit the `err_is_pty_session_missing`
        // race arm — if so, the post-loop fall-through must still promote
        // spawn_with_resume so the on-disk transcript isn't abandoned. Same
        // outcome as grinch G.H2's phantoms-only path, just a different
        // upstream cause (race-killed siblings vs. desync phantoms).
        let mut lost_inject_to_race = false;

        // Issue #223: enumerate ALL viable CWD candidates (PTY-liveness filtered)
        // with their captured status, plus a had_any_match flag for the phantoms-
        // only AC5-preservation path (grinch G.H2). Iterate so a stale-Running
        // phantom no longer blocks delivery to the live Idle recipient at the
        // same CWD; defer Exited destroys until later Inject attempts fail
        // (grinch G.H1) — preserves AC5's "first Exited wins respawn slot".
        //
        // #293: Root Agent recipient uses the `is_root_agent` flag lookup,
        // not CWD-FQN match, since `agent_fqn_from_path(ac-root-agent)` does
        // not produce `ROOT_AGENT_SENDER`. Exited records are filtered out by
        // `find_root_session_candidate` so the deferred-destroy block never
        // fires on a user-launched session.
        let (candidates, had_any_match) =
            if crate::config::root_agent::is_root_agent_target(&msg.to) {
                match self.find_root_session_candidate(app).await {
                    Some(pair) => (vec![pair], true),
                    None => (Vec::new(), false),
                }
            } else {
                self.find_live_candidates(app, &msg.to).await
            };

        for &(session_id, ref status) in &candidates {
            log::debug!(
                "[mailbox] wake: candidate {} captured-status={:?}",
                session_id,
                status
            );

            match wake_action_for(status) {
                WakeAction::Inject => {
                    match self.inject_wake_into_pty(app, session_id, msg).await {
                        Ok(()) => return Ok(()),
                        Err(e) if err_is_pty_session_missing(&e) => {
                            // Race: PTY died between `find_live_candidates`
                            // probe and `PtyManager::write`. Load-bearing
                            // safety net for the dropped per-iteration
                            // re-read (grinch G.M2). Try the next candidate.
                            // Flag the race for the post-loop AC5 promotion
                            // (grinch HIGH-1) — if every Inject candidate
                            // races to dead, the spawn-persistent fall-through
                            // must still set spawn_with_resume.
                            lost_inject_to_race = true;
                            log::warn!(
                                "[mailbox] wake: candidate {} died after liveness probe ({}), trying next",
                                session_id,
                                e
                            );
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                WakeAction::RespawnExited => {
                    // Defer the destroy: an Inject candidate later in the list
                    // may succeed and avoid destroy+spawn entirely (grinch
                    // G.H1). Only the FIRST Exited's id is remembered —
                    // preserves AC5's "first Exited wins the respawn slot"
                    // semantic.
                    if pending_exited_destroy.is_none() {
                        pending_exited_destroy = Some(session_id);
                        pending_exited_telegram_bot_id = {
                            let session_mgr =
                                app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                            let mgr = session_mgr.read().await;
                            mgr.get_session(session_id)
                                .await
                                .and_then(|s| s.telegram_bot_id.clone())
                        };
                        spawn_with_resume = true;
                        log::debug!(
                            "[mailbox] wake: deferring Exited destroy for {} (status={:?}) pending later Inject success",
                            session_id,
                            status
                        );
                    }
                    continue;
                }
            }
        }

        // No Inject returned Ok. Three cases enable auto-resume on the spawn-
        // persistent fall-through:
        //   1. A deferred Exited destroy is pending (spawn_with_resume true).
        //   2. Candidate list empty BUT records existed in the manager — i.e.,
        //      every CWD-match was a non-Exited phantom. Spawning cold would
        //      abandon any on-disk transcript at the matched CWD (grinch G.H2
        //      AC5 micro-regression).
        //   3. Every viable Inject candidate died mid-flight (all hit the
        //      `err_is_pty_session_missing` race arm). Symmetric to case 2 —
        //      same AC5 outcome since cold spawn would abandon the transcript
        //      (grinch HIGH-1, Step-7 review).
        if pending_exited_destroy.is_none()
            && (candidates.is_empty() && had_any_match || lost_inject_to_race)
        {
            spawn_with_resume = true;
            log::debug!(
                "[mailbox] wake: all CWD candidates for '{}' are phantoms or lost to race; enabling auto-resume on spawn-persistent",
                msg.to
            );
        }

        if let Some(exited_id) = pending_exited_destroy {
            log::debug!(
                "[mailbox] wake: executing deferred destroy for Exited candidate {}",
                exited_id
            );
            if let Err(e) = self.destroy_exited_wake_session(app, exited_id).await {
                // Best-effort. Orphan SessionManager record lingers until AC3's
                // runtime-dedup path (`#223-fu1`) drains it. spawn-persistent
                // below still fires with the correct spawn_with_resume flag.
                // (grinch G.M5 option (c).)
                log::error!(
                    "[mailbox] wake: failed to destroy exited session {} (orphan will linger until AC3): {}",
                    exited_id,
                    e
                );
            }
        }

        // ── No viable Inject candidate succeeded — spawn a persistent one ──
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );

        // #293: Root Agent is user-launched; never auto-spawn or destroy a
        // root session implicitly via this path. Reject with an explicit
        // message instead. The Exited filter in `find_root_session_candidate`
        // already preserves the user's session record (the deferred-destroy
        // block above does not fire because the Exited candidate was never
        // returned).
        if crate::config::root_agent::is_root_agent_target(&msg.to) {
            // Soft-handle the daemon-restart window: if the SessionManager is
            // still restoring sessions, the root may be on its way back. We
            // do NOT pin this rejection as `ERR_UNRESOLVABLE_AGENT`-class, so
            // the retry tracker keeps cycling and the message redelivers on
            // the next poll once the root session reappears.
            let restoring = app
                .state::<Arc<crate::RestoreInProgress>>()
                .0
                .load(std::sync::atomic::Ordering::SeqCst);
            return if restoring {
                Err(format!(
                    "Root Agent session not yet restored for '{}'; daemon restart in progress — will retry.",
                    msg.to
                ))
            } else {
                Err(format!(
                    "No live Root Agent session for '{}'. The Root Agent must be running locally to receive messages — ask the user to launch it.",
                    msg.to
                ))
            };
        }

        let resolved_command = self.resolve_agent_command(app, msg).await?;
        let resolved_command = resolved_command.ok_or_else(|| {
            format!(
                "No agent command resolved for '{}'; preferredAgent={:?}. Configure lastCodingAgent or agents in settings.",
                msg.to, msg.preferred_agent
            )
        })?;

        let dest_path = self.resolve_repo_path(&msg.to, app).await;
        let cwd = match dest_path {
            Some(path) => path,
            None => {
                // Fallback: for WG agents (wg-name/agent), derive path from sibling session CWDs
                self.resolve_wg_path_from_sessions(app, &msg.to)
                    .await
                    .ok_or_else(|| {
                        format!(
                            "Cannot resolve repo path for '{}' — cannot spawn session",
                            msg.to
                        )
                    })?
            }
        };

        // §AR2-session-name: strip optional `<project>:` prefix from the display
        // name so the sidebar label stays short (e.g. "wg-1-devs/tech-lead" not
        // "proj-a:wg-1-devs/tech-lead"). The canonical FQN stays recoverable via
        // `agent_fqn_from_path(&cwd)` at any list-sessions time.
        let session_name = {
            let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
            local.to_string()
        };

        let resolved_spawn = if let Some(aid) = resolved_command.agent_id.as_deref() {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            crate::commands::session::build_configured_agent_spawn_for_cwd(&cfg, aid, &cwd, None)?
        } else {
            None
        };
        let (spawn_shell, spawn_args, spawn_label) = if let Some(spawn) = resolved_spawn.as_ref() {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (
                resolved_command.shell.clone(),
                resolved_command.shell_args.clone(),
                resolved_command.agent_label.clone(),
            )
        };
        let spawn_source = resolved_command.source.clone();
        let spawn_raw = resolved_command.raw_command.clone();

        log::info!(
            "[mailbox] wake: spawning '{}' from {}: raw_command={:?}, shell={:?}, args={:?}",
            msg.to,
            spawn_source,
            spawn_raw,
            spawn_shell,
            spawn_args
        );

        let info = self
            .spawn_wake_session(
                app,
                msg,
                &resolved_command,
                cwd,
                session_name,
                spawn_with_resume,
                spawn_shell.clone(),
                spawn_args.clone(),
                spawn_label,
                resolved_spawn,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to spawn session for '{}': command from {} resolved raw_command={:?}, shell={:?}, args={:?}: {}",
                    msg.to, spawn_source, spawn_raw, spawn_shell, spawn_args, e
                )
            })?;

        let session_id =
            Uuid::parse_str(&info.id).map_err(|e| format!("Failed to parse session id: {}", e))?;

        if pending_exited_telegram_bot_id.is_some() {
            self.attach_persisted_telegram_for_wake(
                app,
                session_id,
                pending_exited_telegram_bot_id.as_deref(),
            )
            .await;
        }

        self.wait_for_spawned_wake_idle(app, session_id).await?;

        // Inject message — interactive mode (session persists, user sees reply instructions)
        self.inject_wake_into_pty(app, session_id, msg).await
    }

    async fn has_pty_session_for_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        id: Uuid,
    ) -> bool {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let scripted = {
                let presence = hooks.pty_presence.lock().unwrap();
                presence.get(&id).copied()
            };
            if let Some(has_pty) = scripted {
                return has_pty;
            }
        }

        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        let pty = pty_mgr.lock().unwrap();
        pty.has_session(id)
    }

    async fn inject_wake_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            {
                let mut calls = hooks.inject_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Inject(session_id));
            }
            let result = {
                let mut results = hooks.inject_results.lock().unwrap();
                results.pop_front()
            };
            return result.unwrap_or(Ok(()));
        }

        let result = self.inject_into_pty(app, session_id, msg, true).await;
        // #552 auto-close: a successful inter-agent wake is activity for the
        // recipient team's silence clock (NOT the badge; inter-agent is not a
        // user message). This single site covers both wake paths (deliver_wake
        // and the spawned-wake path), which both funnel through here. The
        // recipient's own response output would also reset silence via the read
        // loop; recording the delivery makes inter-agent traffic count at once.
        if result.is_ok() {
            if let Some(idle) =
                app.try_state::<std::sync::Arc<crate::pty::idle_detector::IdleDetector>>()
            {
                idle.touch_silence(session_id);
            }
        }
        result
    }

    async fn destroy_exited_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            {
                let mut calls = hooks.destroy_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Destroy(session_id));
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr
                .destroy_session(session_id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
        }

        crate::commands::session::destroy_session_inner(app, session_id).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        resolved_command: &ResolvedWakeAgentCommand,
        cwd: String,
        session_name: String,
        spawn_with_resume: bool,
        spawn_shell: String,
        spawn_args: Vec<String>,
        spawn_label: Option<String>,
        resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    ) -> Result<SessionInfo, String> {
        #[cfg(not(test))]
        let _ = msg;
        let skip_auto_resume = wake_spawn_skip_auto_resume(spawn_with_resume);

        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let call = MailboxSpawnCall {
                to: msg.to.clone(),
                session_name: session_name.clone(),
                cwd: cwd.clone(),
                shell: spawn_shell.clone(),
                shell_args: spawn_args.clone(),
                skip_auto_resume,
            };
            {
                let mut calls = hooks.spawn_calls.lock().unwrap();
                calls.push(call.clone());
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Spawn(call));
            }

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let session = mgr
                .create_session(
                    spawn_shell.clone(),
                    spawn_args.clone(),
                    cwd,
                    resolved_spawn
                        .as_ref()
                        .map(|spawn| spawn.trusted_agent_id.clone())
                        .or_else(|| resolved_command.agent_id.clone()),
                    spawn_label.clone(),
                    Vec::<SessionRepo>::new(),
                    false,
                )
                .await
                .map_err(|e| e.to_string())?;
            mgr.rename_session(session.id, session_name)
                .await
                .map_err(|e| e.to_string())?;
            mgr.mark_idle(session.id).await;
            let inserted = mgr
                .get_session(session.id)
                .await
                .ok_or_else(|| format!("Session {} not found after test spawn", session.id))?;
            return Ok(SessionInfo::from(&inserted));
        }

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            spawn_shell,
            spawn_args,
            cwd,
            Some(session_name),                // readable name, no [temp] prefix
            resolved_command.agent_id.clone(), // links to agent config
            spawn_label,                       // human-readable label
            false,            // skip_tooling_save = false -> persist lastCodingAgent
            Vec::new(),       // git_repos
            skip_auto_resume, // see deliver_wake top
            resolved_spawn,
        )
        .await
    }

    async fn attach_persisted_telegram_for_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        bot_id: Option<&str>,
    ) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let bot_id = bot_id.map(str::to_string);
            {
                let mut calls = hooks.attach_calls.lock().unwrap();
                calls.push((session_id, bot_id.clone()));
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Attach { session_id, bot_id });
            }
            return;
        }

        #[cfg(not(test))]
        {
            crate::commands::session::attach_persisted_telegram_if_configured(
                app, session_id, bot_id,
            )
            .await;
        }
        #[cfg(test)]
        {
            let _ = (app, session_id, bot_id);
        }
    }

    async fn wait_for_spawned_wake_idle<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self.test_hooks.is_some() {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.mark_idle(session_id).await;
            return Ok(());
        }

        // #611: require SUSTAINED idle before injecting. A freshly spawned agent
        // (notably Claude) can hit a quiet window > idle_threshold mid-startup and
        // be marked idle before its TUI input/paste state is stable. Injecting
        // then lands the body in the box but the submit \r can be dropped, so the
        // message is written yet never sent. Waiting for idle to hold for a settle
        // window lets late startup renders finish first; the existing double-Enter
        // in pty/inject.rs then submits reliably. This adds ~settle latency ONLY on
        // the cold-spawn path (wake-active via WakeAction::Inject never calls this).
        let max_wait = std::time::Duration::from_secs(90);
        let poll = std::time::Duration::from_millis(500);
        let settle = std::time::Duration::from_millis(2000);
        let start = std::time::Instant::now();
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

        // Instant the session was first observed continuously idle; reset to None
        // whenever it flips back to busy (a late startup render). Threaded through
        // the pure `next_sustained_idle_state` so the settle/reset policy is tested.
        let mut idle_since: Option<std::time::Instant> = None;

        loop {
            if start.elapsed() >= max_wait {
                log::warn!(
                    "[mailbox] wake: timeout waiting for session {} to reach sustained idle; injecting anyway",
                    session_id
                );
                break; // inject anyway as fallback
            }
            tokio::time::sleep(poll).await;

            // Read the flag under a short-lived lock; never hold it across the
            // pure-decision call below.
            let waiting = {
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => s.waiting_for_input,
                    None => {
                        return Err(format!(
                            "Session {} was destroyed before message injection",
                            session_id
                        ));
                    }
                }
            };

            let was_settling = idle_since.is_some();
            let (next_idle_since, should_inject) =
                next_sustained_idle_state(waiting, idle_since, std::time::Instant::now(), settle);
            idle_since = next_idle_since;

            if should_inject {
                log::info!(
                    "[mailbox] wake: session {} idle sustained for >={}ms, injecting message",
                    session_id,
                    settle.as_millis()
                );
                break;
            }
            if was_settling && !waiting {
                log::info!(
                    "[mailbox] wake: session {} went busy during settle; re-waiting for sustained idle",
                    session_id
                );
            }
        }
        Ok(())
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true (all remaining callers): live interactive `wake`
    /// delivery — plain message only, no response markers, no watcher.
    /// `interactive` = false is currently unreachable (the former
    /// `wake-and-sleep` non-interactive path was removed in 0.7.0). The
    /// `use_markers=true` branch below is retained for future non-interactive
    /// consumers; see _plans/delete-modes.md §2.4.
    async fn inject_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
    ) -> Result<(), String> {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        // ── Remote command path: delegate to the canonical injector ──
        // `/clear` and `/compact` are submitted to the agent's PTY exactly the
        // same way as a normal message body: through `inject_text_into_session`,
        // which owns shell detection and the agent-specific double-Enter safety
        // net (see pty/inject.rs). The injector — not this branch — appends the
        // Enter(s); we pass just the `/command` text.
        if let Some(ref command) = msg.command {
            const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
            if !ALLOWED_COMMANDS.contains(&command.as_str()) {
                return Err(format!("Unsupported remote command '{}'", command));
            }

            // Precondition: agent must be idle (waiting_for_input) AND the
            // shell must be a coding-agent CLI that owns explicit-Enter
            // handling in the canonical injector. The shell check guards
            // against silent failure on non-agent shells (plain bash/pwsh)
            // and on cmd-wrapped Codex sessions: under R1 the injector
            // sends ZERO carriage returns when `needs_explicit_enter` is
            // false, so writing `/clear` into such a shell would leave the
            // text un-submitted and let subsequent user input concatenate
            // with it. Reject explicitly instead — closes grinch's G1 + G3.
            // Removing this reject requires extending `needs_explicit_enter`
            // to recognize the rejected case as agent-aware first; see
            // `#233-followup-cmd-wrapper`.
            {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    None => {
                        return Err(format!(
                            "Session {} not found — cannot execute remote command '{}'",
                            session_id, command
                        ))
                    }
                    Some(s) if !crate::pty::inject::needs_explicit_enter(&s.shell) => {
                        return Err(format!(
                            "Cannot execute remote command '/{}': session shell '{}' is not a coding-agent CLI (Claude / Codex / Gemini / Cursor agent). cmd / pwsh wrappers around an agent are tracked separately as #233-followup-cmd-wrapper.",
                            command, s.shell
                        ))
                    }
                    Some(s) if !s.waiting_for_input => {
                        return Err(format!(
                            "Cannot execute remote command '{}': agent is busy (not idle)",
                            command
                        ))
                    }
                    _ => {} // idle agent-shell — proceed
                }
            }

            // Submit `/<command>` via the canonical text-block injector.
            //
            // TOCTOU note (round-2 correction): the gap between the idle check
            // above and the FINAL `\r` is now ~2 s (text → 1500 ms → \r →
            // 500 ms → \r inside `inject_text_into_session`), not microseconds
            // as the original direct-write path. Two things can interleave
            // inside that window:
            //   1. User keystrokes from xterm.js — they take the path
            //      `frontend → invoke("pty_write") → commands/pty.rs::pty_write
            //      → PtyManager::write` (raw bytes, bypasses the injector). A
            //      user typing between the staggered Enters concatenates with
            //      the un-Enter'd `/<command>`.
            //   2. The idle detector flipping `waiting_for_input` to false
            //      based on independent PTY output, leaving the second `\r`
            //      to land on a busy agent (harmless — at worst an extra
            //      Enter on empty input, per pty/inject.rs:78-79).
            // We accept this race because the standard-message path at
            // mailbox.rs:972 already exhibits it identically and the
            // staggered double-Enter is the empirically-tuned defense
            // against the dominant single-`\r`-eaten failure mode that
            // motivated #233.
            let cmd_text = format!("/{}", command); // NO trailing \r — the injector adds it
            crate::pty::inject::inject_text_into_session(app, session_id, &cmd_text)
                .await
                .map_err(|e| {
                    log::error!(
                        "[mailbox] PTY injection FAILED for command '/{}' session={} msg={}: {}",
                        command,
                        session_id,
                        msg.id,
                        e
                    );
                    e
                })?;

            log::info!(
                "Executed remote command '{}' on session {} (from: {})",
                command,
                session_id,
                msg.from
            );

            let _ = tauri::Emitter::emit(
                app,
                "message_delivered",
                serde_json::json!({
                    "id": msg.id,
                    "from": msg.from,
                    "to": msg.to,
                    "mode": msg.mode,
                    "command": command,
                    "injected": true
                }),
            );

            // Post-command background work:
            //  - `/clear` and `/compact` both keep the still-live child process environment.
            //  - Credentials are env-only; nothing is re-sent through the PTY here.
            //  - If the message has a follow-up body, inject it after the agent becomes idle.
            // Never block the delivery pipeline — spawn as a detached task.
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            tauri::async_runtime::spawn(async move {
                if !msg_clone.body.is_empty() {
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!(
                            "[mailbox] Failed to inject follow-up after /{} for session {}: {}",
                            command_owned,
                            session_id,
                            e
                        );
                    }
                }
            });

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;

        // Interactive and marker-less paths share the minimal PTY wrap via
        // `format_pty_wrap` (single source with `PTY_WRAP_FIXED` used by the
        // CLI clamp). Only the `--get-output` + `request_id` case wraps the
        // payload with response markers.
        let payload = match (use_markers, msg.request_id.as_ref()) {
            (true, Some(rid)) => format!(
                "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
                msg.from, msg.body, rid, rid
            ),
            _ => crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body),
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr
                        .lock()
                        .map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        // SECURITY: this `first_100` log MUST NOT see credential values.
        // Credentials are env-only and must never be routed through PTY payloads.
        // Keep this log limited to standard message payloads.
        log::debug!(
            "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
            session_id,
            msg.id,
            payload.len(),
            payload.chars().take(100).collect::<String>()
        );
        crate::pty::inject::inject_text_into_session(app, session_id, &payload)
            .await
            .map_err(|e| {
                log::error!(
                    "[mailbox] PTY injection FAILED session={} msg={}: {}",
                    session_id,
                    msg.id,
                    e
                );
                e
            })?;

        log::info!(
            "[mailbox] PTY injection SUCCESS session={} msg={}",
            session_id,
            msg.id
        );
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": msg.mode,
                "injected": true
            }),
        );
        Ok(())
    }

    /// Wait for agent to become idle after a remote command, then inject body as follow-up.
    /// Static method — can be spawned as a detached task without borrowing self.
    async fn inject_followup_after_idle_static<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for agent to become idle after remote command ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {} // busy — keep polling
                None => {
                    return Err(format!(
                        "Session {} destroyed before follow-up could be injected",
                        session_id
                    ))
                }
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let payload = crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body);
        crate::pty::inject::inject_text_into_session(app, session_id, &payload).await
    }

    /// Find the best session for a given agent name (matches by working directory).
    /// Prefers active/running non-temp sessions over idle/exited ones.
    ///
    /// Used ONLY by stale-token logging (mailbox.rs:456, 501). Routing now uses
    /// `find_live_candidates` — do not add new callers. (grinch G.L5.)
    async fn find_active_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Option<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        log::debug!(
            "[mailbox] find_active_session for '{}' — {} sessions: {:?}",
            agent_name,
            sessions.len(),
            sessions
                .iter()
                .map(|s| format!(
                    "{}={} status={:?} name={}",
                    s.id, s.working_directory, s.status, s.name
                ))
                .collect::<Vec<_>>()
        );

        // §AR2-G2: exact-FQN filter. Post-§AR2-norm, `agent_name` is canonical
        // (or a legacy form that genuinely matches only one project's CWD). The
        // substring/suffix fuzziness from the pre-fix code is gone — cross-project
        // leakage is impossible at this layer.
        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        if matches.is_empty() {
            log::warn!("[mailbox] No session matched for '{}'", agent_name);
            return None;
        }

        log::debug!(
            "[mailbox] {} CWD matches for '{}': {:?}",
            matches.len(),
            agent_name,
            matches
                .iter()
                .map(|s| format!("{}({})", s.id, s.name))
                .collect::<Vec<_>>()
        );

        // Sort: non-temp first (false < true), then Active/Running before Idle before Exited
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let best = &matches[0];
        log::debug!(
            "[mailbox] Best match for '{}': session {} (name='{}', status={:?})",
            agent_name,
            best.id,
            best.name,
            best.status
        );
        Uuid::parse_str(&best.id).ok()
    }

    /// Find ALL viable CWD-matched candidates for wake delivery, sorted by
    /// preference (Active/Running first, then Idle, then Exited; non-temp before
    /// temp). Filters out records whose `SessionStatus` is non-`Exited` but
    /// whose PtyManager entry is missing — those are desync phantoms (issue
    /// #223). `Exited(_)` candidates are RETAINED for the respawn path.
    ///
    /// Returns `(viable, had_any_match)`:
    /// - `viable`: viable candidates with captured `SessionStatus` so the caller
    ///   can decide Inject-vs-RespawnExited without a second `list_sessions`
    ///   scan per candidate (grinch G.M2 / dev-rust R1.B3 alt). The captured
    ///   status may be stale by the time the caller acts on it; the
    ///   `err_is_pty_session_missing` continue arm in `deliver_wake` is the
    ///   load-bearing safety net for that race.
    /// - `had_any_match`: true if at least one CWD-matched record existed BEFORE
    ///   the predicate filter — the caller uses this to distinguish "no record
    ///   at all" (cold spawn) from "phantoms only" (warm spawn with auto-resume,
    ///   preserves on-disk Claude/Codex/Gemini transcript). (grinch G.H2.)
    async fn find_live_candidates<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> (Vec<(Uuid, SessionStatus)>, bool /* had_any_match */) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        let had_any_match = !matches.is_empty();
        if !had_any_match {
            return (Vec::new(), false);
        }

        // Same sort key as the legacy `find_active_session`.
        // TODO(#223-fu1): once AC3's PTY-exit hook lands and starts emitting
        // non-zero exit codes, add a tertiary tie-break (e.g. created_at) so
        // Exited-within-bucket ordering is deterministic. (grinch G.L2.)
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let candidates: Vec<(Uuid, SessionStatus, String)> = matches
            .iter()
            .filter_map(|s| {
                Uuid::parse_str(&s.id)
                    .ok()
                    .map(|id| (id, s.status.clone(), s.name.clone()))
            })
            .collect();
        drop(mgr);

        let mut viable = Vec::new();
        for (id, status, name) in candidates {
            let has_pty = self.has_pty_session_for_wake(app, id).await;
            if !is_viable_wake_candidate(&status, has_pty) {
                // Phantom skip — observable in prod logs to scope the AC3
                // follow-up. (dev-rust R1.B4.)
                log::warn!(
                    "[mailbox] skipping desync phantom: id={} status={:?} has_pty={} name='{}'",
                    id,
                    status,
                    has_pty,
                    name
                );
            } else {
                viable.push((id, status));
            }
        }

        log::debug!(
            "[mailbox] {} viable wake candidate(s) for '{}' (had_any_match={}): {:?}",
            viable.len(),
            agent_name,
            had_any_match,
            viable
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>(),
        );
        (viable, had_any_match)
    }

    /// Locate the live Root Agent session for routing `msg.to == ROOT_AGENT_SENDER`.
    ///
    /// Returns the first match by `SessionManager` iteration order. Persistence
    /// dedup (`config/sessions_persistence.rs:242-329`) converges to a single
    /// root session at steady state, but multiple records may exist transiently
    /// during concurrent spawns — a defensive log fires when more than one
    /// matches so future code does not silently rely on "exactly one" as a
    /// structural invariant.
    ///
    /// Filters via `is_viable_root_recipient` (stricter than
    /// `is_viable_wake_candidate`) — Exited records are returned as `None` so
    /// the caller's no-spawn guard sees `(empty, false)` instead of triggering
    /// the destroy-and-respawn path on a user-launched session.
    async fn find_root_session_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Option<(Uuid, SessionStatus)> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let matches: Vec<_> = sessions
            .iter()
            .filter(|s| {
                s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
            })
            .collect();
        if matches.len() > 1 {
            log::warn!(
                "[mailbox] root recipient: {} root sessions found ({}); routing to first. Persistence dedup should converge.",
                matches.len(),
                matches
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let candidate = matches.into_iter().next()?;

        let id = Uuid::parse_str(&candidate.id).ok()?;
        let pty = pty_mgr.lock().unwrap();
        let has_pty = pty.has_session(id);
        drop(pty);

        if !is_viable_root_recipient(&candidate.status, has_pty) {
            log::warn!(
                "[mailbox] root recipient: not viable (id={} status={:?} has_pty={}); preserving record, no destroy",
                id,
                candidate.status,
                has_pty
            );
            return None;
        }

        Some((id, candidate.status.clone()))
    }

    /// Find ALL sessions matching an agent name (by working directory).
    /// Returns all matching session UUIDs, not just the "best" one.
    ///
    /// §AR2-G2: exact-FQN filter (same simplification as `find_active_session`).
    async fn find_all_sessions<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Vec<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        filter_sessions_by_fqn(&sessions, agent_name)
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
    }

    /// Handle close-session action: validate coordinator auth, find target sessions, destroy them.
    ///
    /// NEW ACTION HANDLERS: resolve user-supplied target fields via
    /// `config::teams::resolve_agent_target` BEFORE privileged operations.
    /// The outbox is a trust boundary — any new destructive action must
    /// canonicalize its target here, not rely on CLI-side resolution.
    ///
    /// §224 G-IMPL-2 — `is_app_outbox`: true when the message file lives
    /// under the instance-private app-outbox (master/root-token path).
    /// Used by the response-write block (A.6) to skip the outbox-relative
    /// primary write — for app-outbox messages it would land under
    /// `<config_dir>/instances/<id>/responses/`, a directory the CLI does
    /// not poll, leaking orphan JSON files with no GC.
    async fn handle_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let raw_target = msg
            .target
            .as_deref()
            .ok_or_else(|| "close-session requires 'target' field".to_string())?;

        // §AR2-G1: resolve the target to a canonical FQN BEFORE authorization.
        // Even if the CLI skipped resolution (direct outbox write, old client,
        // hand-crafted JSON), the mailbox is the authoritative gate.
        let resolved_target = {
            let paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match crate::config::teams::resolve_agent_target(raw_target, &paths) {
                Ok(fqn) => fqn,
                Err(e) => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!("close-session target unresolvable: {}", e),
                        )
                        .await;
                }
            }
        };
        let target = resolved_target.as_str();

        // Re-check master token for coordinator auth bypass (independent of routing bypass above)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        if !is_master {
            let discovered = teams::discover_teams();
            if !teams::is_coordinator_of(&msg.from, target, &discovered) {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "Not authorized: '{}' is not a coordinator of '{}' team",
                            msg.from, target
                        ),
                    )
                    .await;
            }
        }

        // Find all sessions for the target agent.
        //
        // §224 A.2 — empty `session_ids` is NOT an error. The pre-fix code
        // rejected with "No active session found", which conflicted with
        // `list-sessions` reporting the session as alive (ghost rows from
        // `persist_merging_failed`; see A.1). Now we fall through to a
        // successful no-op response with status="no_match".
        //
        // §224 A.2.5 — daemon-restart race guard. If the initial probe is
        // empty AND restore is still in progress, poll up to 5s for either
        // (a) the flag to clear, or (b) a matching session to appear. Three
        // outcomes:
        //   * session appears   → fall through to the kill loop, status="closed".
        //   * flag clears empty → fall through to no_match path.
        //   * 5s elapses, flag still set → restore_in_progress_result = true.
        let mut session_ids = self.find_all_sessions(app, target).await;
        let mut restore_in_progress_result = false;
        if session_ids.is_empty() {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let poll = std::time::Duration::from_millis(100);
                // We can't pass `self.find_all_sessions(...)` as a closure
                // because of the `&self` capture + `async` future shape, so
                // inline the wait loop instead of going through the pure helper.
                // The helper's logic is unit-tested separately (D.5a).
                loop {
                    if std::time::Instant::now() >= deadline {
                        if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                            restore_in_progress_result = true;
                        }
                        break;
                    }
                    tokio::time::sleep(poll).await;
                    session_ids = self.find_all_sessions(app, target).await;
                    if !session_ids.is_empty() {
                        break;
                    }
                    if !restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                        // Flag cleared mid-wait. One final probe (the restore
                        // task may have just inserted our target as its last
                        // act before the guard dropped), then fall through.
                        session_ids = self.find_all_sessions(app, target).await;
                        break;
                    }
                }
            }
        }

        let force = msg.force.unwrap_or(false);
        let timeout_secs = msg.timeout_secs.unwrap_or(30);

        // §224 A.2 — gate the kill-loop log on non-empty session_ids so we
        // don't emit a misleading "force-killing 0 session(s)" line on the
        // no_match / restore_in_progress paths.
        if !session_ids.is_empty() {
            log::info!(
                "[mailbox] close-session: {} {} session(s) for '{}' (requested by '{}', timeout={}s)",
                if force {
                    "force-killing"
                } else {
                    "gracefully closing"
                },
                session_ids.len(),
                target,
                msg.from,
                timeout_secs
            );
        }

        let mut closed_ids: Vec<String> = Vec::new();
        for sid in &session_ids {
            let success = if force {
                self.force_close_session(app, *sid).await
            } else {
                self.graceful_close_session(app, *sid, timeout_secs).await
            };
            if success {
                closed_ids.push(sid.to_string());
            }
        }

        // §224 A.7 — active ghost cleanup. After A.2.5 has confirmed (with
        // wait-and-retry) that no session matches the target, force a
        // sessions.json rewrite from the live SessionManager to drop any
        // stale persisted entry. Without this, a user with only the ghost
        // session sees the contradiction persist across multiple
        // close-session invocations (zero lifecycle events to trigger
        // passive cleanup). Cost: one disk write per no-match call.
        //
        // Skip when A.2.5 returned restore_in_progress: the restore task
        // is itself about to write the snapshot when it completes; racing
        // that write is wasted I/O and would clobber recipes for sessions
        // whose restore is pending.
        //
        // Skip when session_ids is non-empty: that's either "closed" or
        // "already_closed", and the destroy path's downstream events will
        // trigger persist_current_state organically.
        //
        // §224 G-IMPL-4 (NIT, accepted) — snapshot vs concurrent create_session
        // is racy in the ~5-50ms window between `snapshot_sessions` and
        // `save_sessions`. If a `create_session` for any target lands in
        // that window and runs its own `persist_current_state` BEFORE our
        // `save_sessions` writes, the new session is dropped from disk
        // until the next lifecycle event re-persists. Impact: brief disk
        // inconsistency, recoverable. Accepted.
        //
        // §224 G-IMPL-6 (NIT, accepted) — A.7 also drops unrelated failed-
        // recoverable ghosts. The snapshot includes only live SessionManager
        // entries; if `sessions.json` has a failed-recoverable ghost for
        // unrelated agent X, this rewrite drops X's ghost too. This is the
        // pre-existing behavior of every `persist_current_state` caller
        // (see `persist_merging_failed` docstring); A.7 just introduces a
        // new caller outside lifecycle events. Accepted.
        if session_ids.is_empty() && !restore_in_progress_result {
            // Persist through the serialized snapshot+write path so this cleanup
            // cannot replay a stale snapshot after a completed Telegram toggle.
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            if let Err(e) =
                crate::config::sessions_persistence::persist_current_state_result(&mgr).await
            {
                log::warn!(
                    "[mailbox] close-session: failed to persist cleaned sessions.json after no_match: {}",
                    e
                );
            }
        }

        // Write response with details.
        //
        // §224 A.2 + A.2.5 — four terminal states, all exit-0 from the user's
        // seat:
        //   "restore_in_progress" — daemon still in startup; couldn't decide.
        //   "no_match"            — found zero live sessions matching the FQN.
        //   "already_closed"      — found some, but every one vanished before
        //                           destroy (race).
        //   "closed"              — at least one was actively killed.
        if let Some(ref rid) = msg.request_id {
            let status = if restore_in_progress_result {
                "restore_in_progress"
            } else if session_ids.is_empty() {
                "no_match"
            } else if closed_ids.is_empty() {
                "already_closed"
            } else {
                "closed"
            };
            let response = serde_json::json!({
                "action": "close-session",
                "target": target,
                "status": status,
                "sessions_closed": closed_ids.len(),
                "session_ids": closed_ids,
                "requested_by": msg.from,
            });
            let json = match serde_json::to_string_pretty(&response) {
                Ok(j) => j,
                Err(e) => {
                    log::warn!(
                        "[mailbox] Failed to serialize close-session response: {}",
                        e
                    );
                    return self.move_to_delivered(path, msg).await;
                }
            };

            // §224 A.6 — dual-write the response:
            //
            // (1) <message_file_dir>/../responses/<rid>.json — always derivable
            //     from `path` (the queued message's file location). This is
            //     exactly `<ac_dir>/responses/<rid>.json`, the CLI's polled
            //     location (close_session.rs:194-195), so no resolve_repo_path
            //     dependency.
            //
            // (2) <resolve_repo_path(msg.from)>/<agent_local_dir>/responses/<rid>.json
            //     — preserves cross-agent delivery for cases where the sender
            //     FQN points to a different ac_dir than the outbox file's
            //     parent. Best-effort; failure does not affect (1).
            //
            // Either write succeeding is enough for the CLI to receive the
            // response.
            //
            // §224 G-IMPL-2 — skip the derived primary write (1) when the
            // message came from the app-outbox (master/root-token path).
            // For app-outbox messages the parent path is
            // `<config_dir>/instances/<id>/outbox/`, so the derived responses
            // dir resolves to `<config_dir>/instances/<id>/responses/` — a
            // directory the CLI never polls (it always polls
            // `<--root>/.<bin_stem>/responses/`). Writing there leaks orphan
            // JSON files with no GC; the resolved-sender path (2) is the only
            // useful target for the master-token case. If `resolve_repo_path`
            // also fails (msg.from not enumerable), the CLI hits the response
            // timeout and exits 2 (G-IMPL-3) — "outcome unknown".
            if !is_app_outbox {
                let outbox_relative_responses_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|ac_dir| ac_dir.join("responses"));
                if let Some(responses_dir) = outbox_relative_responses_dir {
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let response_path = responses_dir.join(format!("{}.json", rid));
                    if let Err(e) = std::fs::write(&response_path, &json) {
                        log::warn!(
                            "[mailbox] Failed to write close-session response to outbox-relative path {:?}: {}",
                            response_path, e
                        );
                    }
                } else {
                    log::warn!(
                        "[mailbox] close-session: cannot derive outbox-relative responses dir from message path {:?}",
                        path
                    );
                }
            }

            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let responses_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name())
                    .join("responses");
                let _ = std::fs::create_dir_all(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::write(&response_path, &json) {
                    log::warn!(
                        "[mailbox] Failed to write close-session response to resolved-sender path: {}",
                        e
                    );
                }
            } else if is_app_outbox {
                // §224 G-IMPL-2 — app-outbox call AND resolve_repo_path failed
                // means the CLI's `--root` is not enumerable in project_paths.
                // Both write paths above are unreachable; the CLI will hit its
                // response-poll timeout and exit 2 ("outcome unknown",
                // G-IMPL-3). Log explicitly so operators can debug.
                log::warn!(
                    "[mailbox] close-session app-outbox response is undeliverable: \
                     resolve_repo_path(msg.from='{}') returned None — the sender's --root \
                     is not enumerable in project_paths. CLI will timeout with exit 2.",
                    msg.from
                );
            }
        }

        // Move original message to delivered/
        self.move_to_delivered(path, msg).await
    }

    /// #617 - queue a deferred self-clear for the session that owns `msg.token`.
    /// Returns fast: the 30s sustained-idle wait runs in a detached task so the
    /// poll loop is never blocked. Idempotent: a second request while one is
    /// pending is a no-op ("already_queued").
    async fn handle_self_clear<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Resolve the caller's OWN session from the token (the sole authority).
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear: no live session owns this token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "self-clear: internal error resolving session id")
                    .await;
            }
        };

        // 2. Shell guard - /clear is only meaningful on a coding-agent CLI, and the
        //    injector only sends explicit Enter for those shells. Same constraint as
        //    send --command clear (cmd/pwsh wrappers tracked under #233-followup-cmd-wrapper).
        if !crate::pty::inject::needs_explicit_enter(&session.shell) {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-clear: session shell '{}' is not a coding-agent CLI (Claude / Codex / Gemini / Cursor agent); /clear is not supported here",
                        session.shell
                    ),
                )
                .await;
        }

        // 2b. #626 existence gate - REFUSE if the agent did not write its handoff notes. Clearing with
        //     no SELF-HANDOFF.md would wipe context with no way to resume (the agent would post-clear
        //     read a nonexistent file = blank). Queue-time intent guard (not transactional): the real
        //     read-time safety net is SELF_CLEAR_HANDOFF_PROMPT's "if missing or empty, wait". Use
        //     .is_file() so a stray directory named SELF-HANDOFF.md does not pass. Runs BEFORE the
        //     idempotency insert and the archive, so nothing is queued/archived with nothing to resume.
        let handoff_path = std::path::Path::new(&session.working_directory).join("SELF-HANDOFF.md");
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-clear: SELF-HANDOFF.md not found in your root ({}); write it before \
                         requesting self-handoff-and-clear.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 3. Idempotency - atomic check-and-set. insert() returns false if already present.
        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        // 4. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the poller).
        //    MED-4: gated under #[cfg(not(test))] so handle_self_clear unit tests assert
        //    queue + idempotency WITHOUT launching a live 500ms poll task. The gate's
        //    decision policy is covered separately by the pure `self_clear_gate_advance` tests.
        //    Durations are passed in so a future integration test can drive it fast.
        if newly_inserted {
            // #626 - archive SELF-FORGET.md so the next cycle starts fresh. Best-effort: a failure must
            // not block the clear/handoff. The agent already wrote SELF-HANDOFF.md (existence-gated above),
            // so SELF-FORGET.md is present iff the agent kept one. FOLD-3: runs in ALL cfgs (NOT inside the
            // cfg(not(test)) spawn) so the harness archive assertion can pass. Decoupled from clear
            // success (queue-time): an abandoned cycle leaves SELF-FORGET archived but uncleared; content is
            // preserved in self-clear/<ts>_SELF-FORGET.md, re-issue continues normally.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-clear: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-clear: SELF-FORGET.md archive failed for session {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_clear_after_sustained_idle(
                        &app_clone,
                        session_id,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                        SELF_HANDOFF_ARCHIVE_DELAY,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-clear queued for session {} (from '{}')",
                session_id,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-clear already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 5. Write the queue-ack response, then move the message to delivered/.
        self.write_self_clear_response(app, path, msg, session_id, status, is_app_outbox)
            .await
    }

    fn session_has_visible_raise_hand_slot(session: &SessionInfo) -> bool {
        if !session.is_coordinator || matches!(&session.status, SessionStatus::Exited(_)) {
            return false;
        }
        let Some(task_path) =
            crate::session::session::find_workgroup_task_path_for_cwd(&session.working_directory)
        else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(task_path) else {
            return false;
        };
        crate::commands::entity_creation::parse_task_title(&content).is_some()
    }

    async fn handle_raise_hand<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "raise-hand requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        let Some(session) = mgr.find_by_token(token_uuid).await else {
            return self
                .reject_message(
                    path,
                    msg,
                    "raise-hand: no live session owns this token; restart or respawn the session",
                )
                .await;
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "raise-hand: internal error resolving session id")
                    .await;
            }
        };

        if !Self::session_has_visible_raise_hand_slot(&session) {
            return self
                .write_raise_hand_response(
                    app,
                    path,
                    msg,
                    session_id,
                    false,
                    "not_visible",
                    is_app_outbox,
                )
                .await;
        }

        // #698 - raise the hand and persist its snapshot atomically with respect
        // to all session persistence (see `raise_hand_and_persist_result`). The
        // mutation, snapshot, and save happen under one global save lock, so no
        // concurrent persist can durably write the raised state before this
        // snapshot lands. The snapshot is durable before we emit the UI event or
        // write the success response, so a peer reading `list-sessions` sees
        // `raisedHand: true` only after it survived. On persist failure the helper
        // has already rolled back the live communication under the same lock, so
        // we just reject the message rather than reporting a success that did not
        // survive.
        let outcome = crate::config::sessions_persistence::raise_hand_and_persist_result(
            &mgr,
            session_id,
            chrono::Utc::now(),
        )
        .await;
        let (raised, status, changed_communication) = match outcome {
            Ok(RaiseHandPersistOutcome::Raised(communication)) => {
                (true, "raised", Some(communication))
            }
            Ok(RaiseHandPersistOutcome::AlreadyVisible) => (true, "already_visible", None),
            Ok(RaiseHandPersistOutcome::NotRaisable) => (false, "not_visible", None),
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!("raise-hand: failed to persist state: {}", e),
                    )
                    .await;
            }
        };

        if let Some(communication) = changed_communication {
            let _ = tauri::Emitter::emit(
                app,
                "session_communication_changed",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "communication": communication,
                }),
            );
        }

        self.write_raise_hand_response(app, path, msg, session_id, raised, status, is_app_outbox)
            .await
    }

    /// #668 - queue a deferred self switch for the session that owns `msg.token`.
    /// It shares the self context pending set with self-clear, so clear and switch
    /// requests cannot stack on the same live session.
    async fn handle_self_handoff_switch<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: no live session owns this token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: internal error resolving session id",
                    )
                    .await;
            }
        };

        if !crate::pty::inject::needs_explicit_enter(&session.shell) {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: session shell '{}' is not a coding-agent CLI (Claude / Codex / Gemini / Cursor agent); switch is not supported here",
                        session.shell
                    ),
                )
                .await;
        }

        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        let session_cwd = PathBuf::from(&session.working_directory);
        let replica_path = match validate_self_switch_wg_replica(&settings_snapshot, &session_cwd) {
            Ok(path) => path,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        let targets = match resolve_switch_targets(
            &settings_snapshot,
            &replica_path,
            msg.switch_coding_agent.as_deref(),
            msg.switch_profile.as_deref(),
            session.agent_id.as_deref(),
            session.effective_profile.as_deref(),
            session.requested_profile.as_deref(),
        ) {
            Ok(targets) => targets,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        if let Err(reason) = validate_self_switch_spawn(&settings_snapshot, &replica_path, &targets)
        {
            return self.reject_message(path, msg, &reason).await;
        }

        let handoff_path = replica_path.join("SELF-HANDOFF.md");
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: SELF-HANDOFF.md not found in your root ({}); write it before requesting self-handoff-and-switch.",
                        replica_path.display()
                    ),
                )
                .await;
        }

        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        if newly_inserted {
            let forgotten_summary = capture_self_forget_summary(&replica_path);
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
            match archive_root_md(&replica_path, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-switch: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {}
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-switch: SELF-FORGET.md archive failed for session {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                let target_agent = targets.coding_agent.clone();
                let target_profile = targets.profile.clone();
                let replica_path = replica_path.clone();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_switch_after_sustained_idle(
                        &app_clone,
                        session_id,
                        replica_path,
                        target_agent,
                        target_profile,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                        SELF_HANDOFF_ARCHIVE_DELAY,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-switch queued for session {} target coding agent '{}' profile '{}' (from '{}')",
                session_id,
                targets.coding_agent,
                targets.profile,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-switch already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        self.write_self_switch_response(app, path, msg, session_id, status, &targets, is_app_outbox)
            .await
    }

    /// #626 - thin timer driver around `self_clear_gate_advance`. Fire-and-forget. Drives BOTH phases
    /// on the stable `session_id` (the PTY and id survive `/clear`), injecting `/clear` then the
    /// handoff prompt, and ALWAYS de-registers on exit. No "inject anyway" fallback - a busy or
    /// never-idle session is never cleared (the user-approved "30s sustained idle" semantic).
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_clear_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        archive_delay: std::time::Duration,
    ) {
        let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            let app = app_for_inject.clone();
            async move { crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_archive = app.clone();
        let archive = move |session_id: Uuid, delay: std::time::Duration| {
            let app = app_for_archive.clone();
            tauri::async_runtime::spawn(async move {
                let root = {
                    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                    let mgr = session_mgr.read().await;
                    mgr.list_sessions()
                        .await
                        .iter()
                        .find(|s| s.id == session_id.to_string())
                        .map(|s| s.working_directory.clone())
                };
                if let Some(root) = root {
                    tokio::time::sleep(delay).await;
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
                    // Best-effort: a rename failure is a non-fatal warn (mirrors the
                    // SELF-FORGET.md archive at queue time).
                    match archive_root_md(std::path::Path::new(&root), "SELF-HANDOFF", &ts) {
                        Ok(Some(p)) => log::info!(
                            "[mailbox] self-handoff-and-clear: archived SELF-HANDOFF.md -> {}",
                            p.display()
                        ),
                        Ok(None) => {} // already gone (agent moved/removed it)
                        Err(e) => log::warn!(
                            "[mailbox] self-handoff-and-clear: SELF-HANDOFF.md archive failed (non-fatal): {}",
                            e
                        ),
                    }
                }
            });
        };

        Self::drive_self_clear_after_sustained_idle(
            session_id,
            pending,
            forgotten_summary,
            settle,
            poll,
            max_defer,
            archive_delay,
            session_state,
            inject,
            archive,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive_self_clear_after_sustained_idle<
        SessionState,
        SessionFut,
        Inject,
        InjectFut,
        Archive,
    >(
        session_id: Uuid,
        pending: Arc<crate::PendingSelfClear>,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        archive_delay: std::time::Duration,
        mut session_state: SessionState,
        mut inject: Inject,
        archive: Archive,
    ) where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        Archive: Fn(Uuid, std::time::Duration) + Send + 'static,
    {
        let mut state = SelfClearGateState::new(std::time::Instant::now());

        loop {
            tokio::time::sleep(poll).await;

            // Presence + waiting flag under a short-lived lock; never held across .await.
            let (present, waiting) = session_state(session_id).await;

            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next; // adopt the advanced state (the Clear->Handoff reset is already applied)

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s; injecting /clear (phase 1)",
                        session_id,
                        settle.as_secs()
                    );
                    // TOCTOU between settle and the final \r is accepted, identical to the
                    // existing send --command clear path.
                    if let Err(e) = inject(session_id, "/clear".to_string()).await {
                        log::warn!(
                            "[mailbox] self-handoff-and-clear: /clear injection failed for session {}: {}",
                            session_id,
                            e
                        );
                        break; // abandon the handoff if the clear could not even be sent
                    }
                    continue; // state is already Phase 2 with reset clocks
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s post-clear; injecting handoff prompt (phase 2)",
                        session_id,
                        settle.as_secs()
                    );
                    // #629 - on a SUCCESSFUL inject the resume prompt is now in, so the handoff file has
                    // served its purpose. Spawn a detached timer that archives SELF-HANDOFF.md ->
                    // self-clear/<ts>_SELF-HANDOFF.md after a grace delay, so a stale handoff file cannot
                    // false-trigger the NEXT cycle's existence gate (which only checks presence). Detached (not inline) so
                    // the de-register below is not delayed by the wait. In-memory only: a daemon restart
                    // inside the window leaves the file (accepted). On inject failure we do NOT archive: the
                    // prompt never reached the agent, so its notes stay at the canonical name for a retry.
                    let prompt = build_self_clear_handoff_prompt(
                        forgotten_summary.as_ref().map(ForgottenSummary::as_str),
                    );
                    match inject(session_id, prompt).await {
                        Ok(_) => archive(session_id, archive_delay),
                        Err(e) => log::warn!(
                            "[mailbox] self-handoff-and-clear: handoff prompt injection failed for session {}: {}",
                            session_id,
                            e
                        ),
                    }
                    break;
                }
                SelfClearGateAction::Abandon(reason) => {
                    // Greppable abandon line so a silently-dropped clear/handoff is diagnosable.
                    // The CLI already warned the caller it is best-effort.
                    log::warn!(
                        "[mailbox] self-handoff-and-clear ABANDONED for session {}: {} (agent may re-issue)",
                        session_id,
                        reason
                    );
                    break;
                }
            }
        }

        // Always de-register (handoff injected / destroy / cap-expiry / clear-inject-fail all land here).
        pending
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_switch_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        archive_delay: std::time::Duration,
    ) {
        let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_persist = app.clone();
        let persist = move |cwd: PathBuf, agent: String, profile: String| {
            let app = app_for_persist.clone();
            async move {
                let settings_snapshot = {
                    let settings = app.state::<SettingsState>();
                    let snapshot = settings.read().await.clone();
                    snapshot
                };
                crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                    &settings_snapshot,
                    &cwd,
                    &agent,
                    &profile,
                )
            }
        };

        let app_for_restart = app.clone();
        let restart = move |session_id: Uuid, agent: String, profile: String| {
            let app = app_for_restart.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
                let settings = app.state::<SettingsState>();
                crate::commands::session::restart_session_inner_with_activation(
                    &app,
                    session_mgr.inner(),
                    pty_mgr.inner(),
                    settings.inner(),
                    session_id,
                    Some(agent),
                    Some(profile),
                    Some(true),
                    true,
                )
                .await
                .map(|info| info.id)
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            let app = app_for_inject.clone();
            async move { crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await }
        };

        let archive = move |root: PathBuf, delay: std::time::Duration| {
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(delay).await;
                let ts = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
                match archive_root_md(&root, "SELF-HANDOFF", &ts) {
                    Ok(Some(p)) => log::info!(
                        "[mailbox] self-handoff-and-switch: archived SELF-HANDOFF.md -> {}",
                        p.display()
                    ),
                    Ok(None) => {}
                    Err(e) => log::warn!(
                        "[mailbox] self-handoff-and-switch: SELF-HANDOFF.md archive failed (non-fatal): {}",
                        e
                    ),
                }
            });
        };

        Self::drive_self_switch_after_sustained_idle(
            original_session_id,
            cwd,
            target_agent,
            target_profile,
            forgotten_summary,
            pending,
            settle,
            poll,
            max_defer,
            archive_delay,
            session_state,
            persist,
            restart,
            inject,
            archive,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive_self_switch_after_sustained_idle<
        SessionState,
        SessionFut,
        Persist,
        PersistFut,
        Restart,
        RestartFut,
        Inject,
        InjectFut,
        Archive,
    >(
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        pending: Arc<crate::PendingSelfClear>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        archive_delay: std::time::Duration,
        mut session_state: SessionState,
        mut persist: Persist,
        mut restart: Restart,
        mut inject: Inject,
        archive: Archive,
    ) where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Persist: FnMut(PathBuf, String, String) -> PersistFut + Send + 'static,
        PersistFut: std::future::Future<Output = Result<(), String>> + Send,
        Restart: FnMut(Uuid, String, String) -> RestartFut + Send + 'static,
        RestartFut: std::future::Future<Output = Result<String, String>> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        Archive: Fn(PathBuf, std::time::Duration) + Send + 'static,
    {
        let mut state = SelfClearGateState::new(std::time::Instant::now());
        let mut session_id = original_session_id;
        let mut new_alias_id: Option<Uuid> = None;

        loop {
            tokio::time::sleep(poll).await;
            let (present, waiting) = session_state(session_id).await;
            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next;

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] self-handoff-and-switch: session {} idle >={}s; persisting target coding agent '{}' profile '{}' and respawning (phase 1)",
                        session_id,
                        settle.as_secs(),
                        target_agent,
                        target_profile
                    );
                    if let Err(e) =
                        persist(cwd.clone(), target_agent.clone(), target_profile.clone()).await
                    {
                        log::warn!(
                            "[mailbox] self-handoff-and-switch: persist failed for session {}: {}",
                            original_session_id,
                            e
                        );
                        break;
                    }
                    let restarted =
                        restart(session_id, target_agent.clone(), target_profile.clone()).await;
                    let new_id = match restarted {
                        Ok(id) => match Uuid::parse_str(&id) {
                            Ok(uuid) => uuid,
                            Err(e) => {
                                log::warn!(
                                    "[mailbox] self-handoff-and-switch: restarted session id '{}' could not be parsed: {}",
                                    id,
                                    e
                                );
                                break;
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "[mailbox] self-handoff-and-switch: restart failed for session {}: {}",
                                original_session_id,
                                e
                            );
                            break;
                        }
                    };
                    {
                        let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
                        if set.insert(new_id) {
                            new_alias_id = Some(new_id);
                        } else {
                            log::warn!(
                                "[mailbox] self-handoff-and-switch: new session {} was already marked pending",
                                new_id
                            );
                        }
                    }
                    session_id = new_id;
                    continue;
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] self-handoff-and-switch: session {} idle >={}s post-switch; injecting handoff prompt (phase 2)",
                        session_id,
                        settle.as_secs()
                    );
                    let prompt = build_self_switch_handoff_prompt(
                        forgotten_summary.as_ref().map(ForgottenSummary::as_str),
                    );
                    match inject(session_id, prompt).await {
                        Ok(_) => archive(cwd.clone(), archive_delay),
                        Err(e) => log::warn!(
                            "[mailbox] self-handoff-and-switch: handoff prompt injection failed for session {}: {}",
                            session_id,
                            e
                        ),
                    }
                    break;
                }
                SelfClearGateAction::Abandon(reason) => {
                    log::warn!(
                        "[mailbox] self-handoff-and-switch ABANDONED for original session {} current session {}: {} (agent may re-issue)",
                        original_session_id,
                        session_id,
                        reason
                    );
                    break;
                }
            }
        }

        let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&original_session_id);
        if let Some(new_id) = new_alias_id {
            set.remove(&new_id);
        }
    }

    /// #617 - write the self-clear queue-ack response, then move the message to
    /// delivered/. Mirrors the close-session dual-write (self-contained copy; the
    /// landed close-session block is intentionally left untouched, blast radius 0).
    async fn write_self_clear_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_CLEAR_ACTION,
                "status": status,                 // "queued" | "already_queued"
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,   // single-sourced const
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                // (1) outbox-relative <ac_dir>/responses/<rid>.json - skip for app-outbox.
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                // (2) resolved-sender path (best-effort).
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_self_switch_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        targets: &SelfSwitchTargets,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_SWITCH_ACTION,
                "status": status,
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,
                "requested_by": msg.from,
                "target_coding_agent": targets.coding_agent,
                "target_profile": targets.profile,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_raise_hand_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        raised: bool,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": RAISE_HAND_ACTION,
                "status": status,
                "raised": raised,
                "session_id": session_id.to_string(),
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(path, msg).await
    }

    /// Force-close a session immediately via destroy_session_inner.
    async fn force_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        sid: Uuid,
    ) -> bool {
        #[cfg(test)]
        {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr.destroy_session(sid).await.is_ok();
        }

        #[cfg(not(test))]
        {
            match crate::commands::session::destroy_session_inner(app, sid).await {
                Ok(()) => {
                    log::info!("[mailbox] close-session: force-destroyed session {}", sid);
                    true
                }
                Err(e) => {
                    log::warn!(
                        "[mailbox] close-session: failed to force-destroy session {}: {}",
                        sid,
                        e
                    );
                    false
                }
            }
        }
    }

    /// Gracefully close a session: inject exit command, poll for Exited, fallback to force on timeout.
    async fn graceful_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        sid: Uuid,
        timeout_secs: u32,
    ) -> bool {
        // Get session info to determine agent type
        let exit_cmd = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) => Self::resolve_exit_command(&s.shell, &s.shell_args),
                None => {
                    log::warn!(
                        "[mailbox] close-session: session {} not found for graceful close",
                        sid
                    );
                    return false;
                }
            }
        };

        log::info!(
            "[mailbox] close-session: injecting '{}' into session {}",
            exit_cmd.escape_debug(),
            sid
        );

        // Inject exit command into PTY.
        // Clone the Arc so the State borrow is released, then lock+write+drop guard before any .await.
        let pty_arc = app
            .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
            .inner()
            .clone();
        let inject_result = match pty_arc.lock() {
            Ok(mgr) => {
                let res = mgr
                    .write(sid, exit_cmd.as_bytes())
                    .map_err(|e| e.to_string());
                drop(mgr);
                res
            }
            Err(e) => Err(format!("PTY lock failed: {}", e)),
        };
        if let Err(e) = inject_result {
            log::warn!(
                "[mailbox] close-session: PTY inject failed for {}: {}, falling back to force",
                sid,
                e
            );
            return self.force_close_session(app, sid).await;
        }

        // Poll for SessionStatus::Exited
        let timeout = std::time::Duration::from_secs(timeout_secs as u64);
        let poll_interval = std::time::Duration::from_secs(1);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= timeout {
                log::warn!(
                    "[mailbox] close-session: graceful timeout ({}s) for session {}, falling back to force",
                    timeout_secs, sid
                );
                return self.force_close_session(app, sid).await;
            }

            tokio::time::sleep(poll_interval).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                    log::info!("[mailbox] close-session: session {} exited gracefully", sid);
                    drop(mgr);
                    // Clean up the exited session
                    return self.force_close_session(app, sid).await;
                }
                None => {
                    // Session already removed
                    log::info!("[mailbox] close-session: session {} already gone", sid);
                    return true;
                }
                _ => {} // still running, keep polling
            }
        }
    }

    /// Determine the exit command to inject based on the session's shell/agent type.
    /// Claude Code -> "/exit\r", generic shell/codex -> "exit\r"
    fn resolve_exit_command(shell: &str, shell_args: &[String]) -> String {
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        let basenames: Vec<String> = full_cmd
            .split_whitespace()
            .map(crate::commands::session::executable_basename)
            .collect();

        if basenames.iter().any(|b| b == "claude" || b == "aider") {
            "/exit\r".to_string()
        } else {
            // Codex, generic shell, and other CLIs
            "exit\r".to_string()
        }
    }

    /// Resolve the full filesystem path for an agent name.
    ///
    /// §AR2-G4: collector pattern. For qualified inputs, an FQN matches at most
    /// one CWD/path/team-member entry per iteration (by construction) — the
    /// dedupe is defense-in-depth for redundant registrations. For unqualified
    /// inputs (legacy), local-part matches across multiple projects return
    /// `None` rather than arbitrarily picking one.
    ///
    /// §AR2-G3: WG fallback seed honors the target project filter. Combined
    /// with §DR2-4 composition: `matches.push(...); break;` within a single
    /// `rp` iteration (FQN can only match one replica dir per project) while
    /// the outer loop continues so cross-project ambiguity is still detected.
    async fn resolve_repo_path<R: tauri::Runtime>(
        &self,
        agent_name: &str,
        app: &tauri::AppHandle<R>,
    ) -> Option<String> {
        let (target_project, target_local) = crate::config::teams::split_project_prefix(agent_name);
        let is_qualified = target_project.is_some();
        let mut matches: Vec<String> = Vec::new();

        let record_match = |path_str: &str, out: &mut Vec<String>| {
            if !out.iter().any(|m| m == path_str) {
                out.push(path_str.to_string());
            }
        };

        let hits_agent = |cwd: &str| -> bool {
            let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
            if is_qualified {
                path_fqn == agent_name
            } else {
                let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
                path_local == target_local
            }
        };

        // Loop 1: session CWDs
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        for (_, cwd) in &dirs {
            if hits_agent(cwd) {
                record_match(cwd, &mut matches);
            }
        }
        drop(mgr);

        // Loop 2: settings project_paths
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        for rp in &cfg.project_paths {
            if hits_agent(rp) {
                record_match(rp, &mut matches);
            }
        }

        // Loop 3: discovered team member paths. Short-circuit by project when
        // the target is qualified (team.project matches target_project).
        let discovered_teams = teams::discover_teams();
        for team in &discovered_teams {
            if let Some(want) = target_project {
                if team.project != want {
                    continue;
                }
            }
            for agent_path in team.agent_paths.iter().flatten() {
                let path_str = agent_path.to_string_lossy().to_string();
                if hits_agent(&path_str) {
                    record_match(&path_str, &mut matches);
                }
            }
        }

        // Loop 4: WG replica fallback. Scan `<workspace>/<wg>/__agent_<short>` under
        // project_paths (base + immediate non-dot children), honoring the target
        // project filter. §DR2-4 composition: push + break within a single `rp`
        // (an FQN matches at most one replica dir per project) but continue the
        // outer loop so ambiguity across projects is detected.
        if target_local.starts_with("wg-") {
            if let Some((wg_name, agent_short)) = target_local.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);

                let project_matches = |dir_name: &str| -> bool {
                    match target_project {
                        Some(want) => dir_name == want,
                        None => true,
                    }
                };

                for rp in &cfg.project_paths {
                    let base = std::path::Path::new(rp);
                    if !base.is_dir() {
                        continue;
                    }
                    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                    if project_matches(base_name) {
                        dirs_to_check.push(base.to_path_buf());
                    }
                    if let Ok(entries) = std::fs::read_dir(base) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if !p.is_dir() {
                                continue;
                            }
                            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with('.') {
                                continue;
                            }
                            if !project_matches(dir_name) {
                                continue;
                            }
                            dirs_to_check.push(p);
                        }
                    }

                    for dir in dirs_to_check {
                        let Some(workspace_dir) =
                            crate::config::workspace::existing_workspace_dir(&dir)
                        else {
                            continue;
                        };
                        let candidate = workspace_dir.join(wg_name).join(&replica_dir);
                        if candidate.is_dir() {
                            record_match(&candidate.to_string_lossy(), &mut matches);
                            // Within a single `rp`, first hit is the unique hit —
                            // an FQN matches one replica dir per project. Continue
                            // OUTER loop to detect cross-project ambiguity (§DR2-4).
                            break;
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => None,
            1 => Some(matches.pop().unwrap()),
            _ => {
                log::warn!(
                    "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: {:?}",
                    agent_name, matches.len(), matches
                );
                None
            }
        }
    }

    // Shadow `agent_name_from_path` removed — all mailbox call sites now use
    // `crate::config::teams::agent_fqn_from_path` per §AR2 (§DR2 consolidation).

    /// Check if sender can reach destination via team membership.
    /// Only agents in the same team can communicate — no parent directory fallback.
    fn can_reach(&self, from: &str, to: &str, discovered_teams: &[teams::DiscoveredTeam]) -> bool {
        crate::config::teams::can_communicate(from, to, discovered_teams)
    }

    /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
    /// persistent session for the destination agent.
    async fn resolve_agent_command<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<Option<ResolvedWakeAgentCommand>, String> {
        let agents = {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            cfg.agents.clone()
        };

        let mut destination_current_agent: Option<String> = None;
        let mut destination_last_agent: Option<String> = None;
        let mut destination_config_path: Option<PathBuf> = None;

        if let Some(dest_path) = self.resolve_repo_path(&msg.to, app).await {
            let dest = Path::new(&dest_path);
            // The Selection UI assignment (currentCodingAgent), including the
            // "Entire Workgroup" path, is written to the replica's top-level
            // config.json by set_replica_coding_agent_selection, not to the
            // per-instance agent_local_dir config that holds lastCodingAgent.
            destination_current_agent =
                crate::config::coding_agent_profiles::read_replica_current_coding_agent(dest);

            let config_path = dest
                .join(crate::config::agent_local_dir_name())
                .join("config.json");
            destination_config_path = Some(config_path.clone());

            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&content) {
                    destination_last_agent = local_config.tooling.last_coding_agent;
                }
            }
        }

        resolve_wake_agent_command_from_sources(
            &agents,
            &msg.preferred_agent,
            destination_current_agent.as_deref(),
            destination_last_agent.as_deref(),
            destination_config_path.as_deref(),
            msg.sender_agent.as_deref(),
        )
    }

    /// Fallback path resolution for WG agents: find a sibling session in the same WG,
    /// derive the WG directory from its CWD, and construct the target agent path.
    ///
    /// §4.4: peel optional `<project>:` prefix before splitting the local part.
    /// If the target is qualified, the returned candidate must also be in the
    /// same project (checked via derived FQN).
    async fn resolve_wg_path_from_sessions<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Option<String> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);

        resolve_wg_path_from_session_dirs(&dirs, agent_name)
    }

    /// Move an outbox message to outbox/delivered/ with token stripped.
    async fn move_to_delivered(&self, path: &Path, msg: &OutboxMessage) -> Result<(), String> {
        let delivered_dir = path.parent().ok_or("No parent dir")?.join("delivered");
        std::fs::create_dir_all(&delivered_dir)
            .map_err(|e| format!("Failed to create delivered dir: {}", e))?;

        // Strip token before storing
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = delivered_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write delivered file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::info!("[mailbox] Message {} moved to delivered/", msg.id);
        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    async fn reject_message(
        &self,
        path: &Path,
        msg: &OutboxMessage,
        reason: &str,
    ) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        // Write reason file FIRST — the CLI polls for this file to detect rejection
        let reason_path = rejected_dir.join(format!("{}.reason.txt", msg.id));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Then write the stripped message JSON
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = rejected_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json).map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::warn!(
            "[mailbox] Message {} moved to rejected/: {}",
            msg.id,
            reason
        );
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as OutboxMessage.
    fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        std::fs::rename(path, &dest)
            .or_else(|_| std::fs::copy(path, &dest).and_then(|_| std::fs::remove_file(path)))
            .map_err(|e| format!("Failed to move file to rejected: {}", e))?;

        let stem = Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let reason_path = rejected_dir.join(format!("{}.reason.txt", stem));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        log::warn!("Rejected raw file {:?}: {}", path, reason);
        Ok(())
    }

    /// Poll ~/.agentscommander/project-refresh-requests/ for sidebar refresh requests.
    async fn poll_project_refresh_requests(&self, app: &tauri::AppHandle) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join("project-refresh-requests");
        let batch = collect_project_refresh_requests(&requests_dir);

        for payload in &batch.payloads {
            log::info!(
                "[project-refresh-requests] Emitting refresh: dir='{}' project='{}' reason='{}'",
                requests_dir.display(),
                payload.project_path,
                payload.reason
            );
            let _ = tauri::Emitter::emit(app, "ac_project_refresh_requested", payload);
        }

        for path in batch.processed_paths {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Poll ~/.agentscommander/session-requests/ for launch requests from the CLI.
    async fn poll_session_requests(&self, app: &tauri::AppHandle) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join("session-requests");
        if !requests_dir.is_dir() {
            return;
        }

        let entries: Vec<PathBuf> = match std::fs::read_dir(&requests_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect(),
            Err(_) => return,
        };

        for path in entries {
            let content = match read_text_bom_tolerant(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[session-requests] Failed to read {:?}: {}", path, e);
                    continue;
                }
            };

            let request: crate::cli::create_agent::SessionRequest =
                match serde_json::from_str(&content) {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!("[session-requests] Failed to parse {:?}: {}", path, e);
                        // Delete malformed file
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                };

            log::info!(
                "[session-requests] Processing: name='{}' cwd='{}' agent='{}'",
                request.session_name,
                request.cwd,
                request.agent_id
            );

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
            let resolved_spawn = {
                let settings = app.state::<SettingsState>();
                let cfg = settings.read().await;
                match crate::commands::session::build_configured_agent_spawn_for_cwd(
                    &cfg,
                    &request.agent_id,
                    &request.cwd,
                    request.requested_profile.as_deref(),
                ) {
                    Ok(spawn) => spawn,
                    Err(e) => {
                        log::error!(
                            "[session-requests] Failed to rebuild configured agent command for '{}': {}",
                            request.session_name,
                            e
                        );
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                }
            };
            let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                (request.shell.clone(), request.shell_args.clone(), None)
            };

            match crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                shell,
                shell_args,
                request.cwd.clone(),
                Some(request.session_name.clone()),
                Some(request.agent_id.clone()),
                agent_label, // No agent label for legacy custom-shell fallback
                false,       // Persist tooling
                Vec::new(),  // git_repos
                true,        // skip_auto_resume = true → CLI session-request is a fresh create
                resolved_spawn,
            )
            .await
            {
                Ok(info) => {
                    log::info!(
                        "[session-requests] Created session '{}' (id={})",
                        request.session_name,
                        info.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[session-requests] Failed to create session '{}': {}",
                        request.session_name,
                        e
                    );
                }
            }

            // Delete processed request file regardless of success/failure
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Read a file as UTF-8 string, tolerant of UTF-8 / UTF-16 LE / UTF-16 BE BOMs.
/// Logs a warning when a BOM is detected so users see which tool is writing
/// odd encoding into outbox / session-requests (typically PowerShell on Windows).
fn read_text_bom_tolerant(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;

    if bytes.starts_with(&[0xFF, 0xFE]) {
        log::warn!(
            "[bom] UTF-16 LE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        log::warn!(
            "[bom] UTF-16 BE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        log::warn!(
            "[bom] UTF-8 BOM detected in {:?} — stripping (writer should use UTF-8 without BOM)",
            path
        );
        String::from_utf8(bytes[3..].to_vec())
            .map_err(|e| format!("Invalid UTF-8 after BOM: {}", e))
    } else {
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;
    use crate::telegram::manager::TelegramBridgeManager;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use tauri::Listener;

    // ── §224 D.5a — wait_for_restore_or_session unit tests ──

    // ── §224 D.3 — filter_sessions_by_fqn pure-predicate tests ──

    fn make_session_info(
        id: &str,
        name: &str,
        cwd: &str,
        status: crate::session::session::SessionStatus,
        waiting_for_input: bool,
    ) -> crate::session::session::SessionInfo {
        crate::session::session::SessionInfo {
            id: id.into(),
            name: name.into(),
            shell: "claude".into(),
            shell_args: vec![],
            effective_shell_args: None,
            created_at: "2026-05-16T00:00:00Z".into(),
            working_directory: cwd.into(),
            status,
            waiting_for_input,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: vec![],
            workgroup_task: None,
            is_coordinator: false,
            is_root_agent: false,
            token: "t".into(),
            agent_kind: Some(crate::session::profile::CodingAgentKind::Claude),
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            profile_content_hash: None,
            profile_outdated: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        }
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }

    fn make_root_route_fixture(
        spoofed_coordinator_identity: bool,
    ) -> (tempfile::TempDir, Vec<String>) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_dev-team");
        let origin_tech_lead = workspace_dir.join("_agent_tech-lead");
        let origin_dev_rust = workspace_dir.join("_agent_dev-rust");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        let tech_lead_identity = if spoofed_coordinator_identity {
            "../../_agent_dev-rust"
        } else {
            "../../_agent_tech-lead"
        };
        std::fs::write(
            tech_lead_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
        )
        .unwrap();
        std::fs::write(
            dev_rust_replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let paths = vec![temp.path().to_string_lossy().to_string()];
        (temp, paths)
    }

    fn root_outbox_message(body: String, command: Option<String>) -> OutboxMessage {
        OutboxMessage {
            id: "msg-root".into(),
            token: None,
            from: crate::config::root_agent::ROOT_AGENT_SENDER.into(),
            to: "proj-a:wg-1-dev-team/tech-lead".into(),
            body,
            mode: "wake".into(),
            get_output: false,
            request_id: None,
            sender_agent: None,
            preferred_agent: "auto".into(),
            priority: "normal".into(),
            timestamp: "2026-05-24T00:00:00Z".into(),
            command,
            action: None,
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: None,
            switch_profile: None,
        }
    }

    fn wake_agent(id: &str, label: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            label: label.into(),
            command: command.into(),
            color: "#10b981".into(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
        }
    }

    fn wake_agents() -> Vec<AgentConfig> {
        vec![
            wake_agent("codex", "Codex", "codex --yolo"),
            wake_agent("claude", "Claude", "claude"),
        ]
    }

    const MAILBOX_MASTER_TOKEN: &str = "mailbox-master-token";
    const CANONICAL_WAKE_FROM: &str = "proj-a:wg-1-dev-team/tech-lead";
    const CANONICAL_WAKE_TO: &str = "proj-a:wg-1-dev-team/dev-rust";
    const LOCAL_WAKE_FROM: &str = "wg-1-dev-team/tech-lead";
    const LOCAL_WAKE_TO: &str = "wg-1-dev-team/dev-rust";
    const WAKE_BODY: &str = "wake body";

    fn make_mailbox_app(projects_root: &Path) -> tauri::App<tauri::test::MockRuntime> {
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));

        let settings = AppSettings {
            project_paths: vec![projects_root.to_string_lossy().to_string()],
            agents: wake_agents(),
            telegram_bots: vec![crate::telegram::types::TelegramBotConfig {
                id: "bot-1".into(),
                label: "Bot 1".into(),
                token: "test-token".into(),
                chat_id: 1,
                color: "#10b981".into(),
            }],
            ..Default::default()
        };

        tauri::test::mock_builder()
            .manage(MasterToken::new(MAILBOX_MASTER_TOKEN.into()))
            .manage(AppOutbox::new(
                projects_root
                    .join(".app-outbox")
                    .to_string_lossy()
                    .to_string(),
            ))
            .manage(Arc::new(tokio::sync::RwLock::new(settings)))
            .manage(session_mgr.clone())
            .manage(Arc::new(tokio::sync::Mutex::new(
                TelegramBridgeManager::new(Arc::new(Mutex::new(HashMap::new()))),
            )))
            .manage(Arc::new(Mutex::new(HashSet::<Uuid>::new())))
            .manage(Arc::new(crate::RestoreInProgress(AtomicBool::new(false))))
            .manage(Arc::new(crate::PendingSelfClear::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mailbox test app")
    }

    fn app_handle(
        app: &tauri::App<tauri::test::MockRuntime>,
    ) -> tauri::AppHandle<tauri::test::MockRuntime> {
        app.handle().clone()
    }

    struct MailboxFixture {
        _temp: tempfile::TempDir,
        sender_cwd: PathBuf,
        target_cwd: PathBuf,
        app: tauri::App<tauri::test::MockRuntime>,
    }

    fn make_mailbox_fixture() -> MailboxFixture {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_dev-team");
        let origin_tech_lead = workspace_dir.join("_agent_tech-lead");
        let origin_dev_rust = workspace_dir.join("_agent_dev-rust");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
        let sender_cwd = wg_dir.join("__agent_tech-lead");
        let target_cwd = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &sender_cwd,
            &target_cwd,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            sender_cwd.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            target_cwd.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let app = make_mailbox_app(temp.path());
        MailboxFixture {
            _temp: temp,
            sender_cwd,
            target_cwd,
            app,
        }
    }

    async fn add_mailbox_session<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        cwd: &Path,
        name: &str,
        status: SessionStatus,
        telegram_bot_id: Option<&str>,
    ) -> Uuid {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr
            .create_session(
                "codex".into(),
                vec!["--yolo".into()],
                cwd.to_string_lossy().to_string(),
                Some("codex".into()),
                Some("Codex".into()),
                Vec::new(),
                false,
            )
            .await
            .unwrap();
        mgr.rename_session(session.id, name.to_string())
            .await
            .unwrap();
        match status {
            SessionStatus::Active => {
                mgr.switch_session(session.id).await.unwrap();
            }
            SessionStatus::Running => {}
            SessionStatus::Idle => {
                mgr.mark_idle(session.id).await;
            }
            SessionStatus::Exited(code) => {
                mgr.mark_exited(session.id, code).await;
            }
        }
        if let Some(bot_id) = telegram_bot_id {
            mgr.set_telegram_bot_id(session.id, Some(bot_id.to_string()))
                .await;
        }
        session.id
    }

    fn write_wake_outbox_message(sender_cwd: &Path, msg_id: &str) -> PathBuf {
        write_wake_outbox_message_with_route(
            sender_cwd,
            msg_id,
            CANONICAL_WAKE_FROM,
            CANONICAL_WAKE_TO,
        )
    }

    fn write_wake_outbox_message_with_route(
        sender_cwd: &Path,
        msg_id: &str,
        from: &str,
        to: &str,
    ) -> PathBuf {
        let outbox_dir = sender_cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        std::fs::create_dir_all(&outbox_dir).unwrap();
        let message_path = outbox_dir.join(format!("{}.json", msg_id));
        let msg = OutboxMessage {
            id: msg_id.into(),
            token: Some(MAILBOX_MASTER_TOKEN.into()),
            from: from.into(),
            to: to.into(),
            body: WAKE_BODY.into(),
            mode: "wake".into(),
            get_output: false,
            request_id: None,
            sender_agent: Some("codex".into()),
            preferred_agent: "codex".into(),
            priority: "normal".into(),
            timestamp: "2026-06-11T00:00:00Z".into(),
            command: None,
            action: None,
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: None,
            switch_profile: None,
        };
        std::fs::write(&message_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
        message_path
    }

    fn assert_no_spawn_or_destroy_events(hooks: &MailboxTestHooks) {
        let events = hooks.events.lock().unwrap().clone();
        assert!(
            events.iter().all(|event| !matches!(
                event,
                MailboxTestEvent::Spawn(_) | MailboxTestEvent::Destroy(_)
            )),
            "unexpected spawn or destroy event: {:?}",
            events
        );
    }

    fn assert_inject_results_consumed(hooks: &MailboxTestHooks) {
        assert!(
            hooks.inject_results.lock().unwrap().is_empty(),
            "all scripted inject results should be consumed"
        );
    }

    fn assert_spawn_call_matches_target(call: &MailboxSpawnCall, fixture: &MailboxFixture) {
        assert_eq!(call.to, CANONICAL_WAKE_TO);
        assert_eq!(call.cwd, fixture.target_cwd.to_string_lossy().to_string());
        assert_eq!(call.session_name, LOCAL_WAKE_TO);
        assert_eq!(call.shell, "codex");
        assert_eq!(call.shell_args, vec!["--yolo"]);
    }

    async fn assert_spawned_session_matches_target<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        fixture: &MailboxFixture,
    ) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let spawned = mgr.get_session(session_id).await.unwrap();
        assert_eq!(
            spawned.working_directory,
            fixture.target_cwd.to_string_lossy().to_string()
        );
        assert_eq!(spawned.name, LOCAL_WAKE_TO);
        assert_eq!(spawned.shell, "codex");
        assert_eq!(spawned.shell_args, vec!["--yolo"]);
        assert!(spawned.waiting_for_input);
    }

    async fn run_mailbox_message<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        message_path: &Path,
        hooks: MailboxTestHooks,
    ) {
        let msg_id = message_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        let delivered = message_path
            .parent()
            .unwrap()
            .join("delivered")
            .join(format!("{}.json", msg_id));
        let poller = MailboxPoller::new_with_test_hooks(hooks);
        poller
            .process_message(app, message_path, false)
            .await
            .expect("process mailbox message");
        assert!(!message_path.exists());
        assert!(delivered.exists());
        let delivered_msg: OutboxMessage =
            serde_json::from_str(&std::fs::read_to_string(&delivered).unwrap()).unwrap();
        assert_eq!(delivered_msg.id, msg_id);
        assert_eq!(delivered_msg.token, None);
        assert_eq!(delivered_msg.from, CANONICAL_WAKE_FROM);
        assert_eq!(delivered_msg.to, CANONICAL_WAKE_TO);
        assert_eq!(delivered_msg.body, WAKE_BODY);
        assert_eq!(delivered_msg.mode, "wake");
    }

    #[test]
    fn wake_agent_command_normalizes_preferred_agent_command() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "codex",
            None,
            Some("claude"),
            None,
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.shell_args, vec!["--yolo"]);
        assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
        assert!(resolved.source.contains("preferredAgent 'codex'"));
    }

    #[test]
    fn wake_agent_command_stale_preferred_agent_falls_back_to_last_coding_agent() {
        let agents = wake_agents();
        let config_path = Path::new("C:/repo/.agentscommander/config.json");

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "missing",
            None,
            Some("codex"),
            Some(config_path),
            Some("claude"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.shell_args, vec!["--yolo"]);
        assert!(resolved.source.contains("lastCodingAgent 'codex'"));
    }

    #[test]
    fn wake_agent_command_normalizes_last_coding_agent_command() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "auto",
            None,
            Some("codex"),
            None,
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.shell_args, vec!["--yolo"]);
        assert!(resolved.source.contains("lastCodingAgent"));
    }

    #[test]
    fn wake_agent_command_uses_sender_agent_after_missing_last_coding_agent() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "auto",
            None,
            Some("missing"),
            None,
            Some("claude"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "claude");
        assert!(resolved.shell_args.is_empty());
        assert_eq!(resolved.agent_id.as_deref(), Some("claude"));
        assert!(resolved.source.contains("senderAgent 'claude'"));
    }

    #[test]
    fn wake_agent_command_uses_first_configured_agent_as_last_resort() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "auto",
            None,
            Some("missing"),
            None,
            Some("also-missing"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.shell_args, vec!["--yolo"]);
        assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
        assert!(resolved.source.contains("first configured agent 'codex'"));
    }

    #[test]
    fn wake_agent_command_prefers_current_coding_agent_over_last() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "auto",
            Some("claude"),
            Some("codex"),
            None,
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "claude");
        assert_eq!(resolved.agent_id.as_deref(), Some("claude"));
        assert!(resolved.source.contains("currentCodingAgent 'claude'"));
    }

    #[test]
    fn wake_agent_command_uses_current_coding_agent_when_last_missing() {
        // The "Entire Workgroup" assignment writes currentCodingAgent to a
        // freshly-spawned member that has no lastCodingAgent yet. Without it the
        // resolver would fall through to senderAgent / first configured agent.
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "auto",
            Some("codex"),
            None,
            None,
            Some("claude"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
        assert!(resolved.source.contains("currentCodingAgent 'codex'"));
    }

    #[test]
    fn wake_agent_command_rejects_invalid_quoted_command_with_source() {
        let agents = vec![wake_agent("codex", "Codex", "codex \"unterminated")];

        let err = resolve_wake_agent_command_from_sources(&agents, "codex", None, None, None, None)
            .unwrap_err();

        assert!(err.contains("preferredAgent 'codex'"));
        assert!(err.contains("agent id 'codex'"));
        assert!(err.contains("label 'Codex'"));
        assert!(err.contains("command=\"codex \\\"unterminated\""));
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn wake_agent_command_selected_agent_preserves_args() {
        let agents = wake_agents();

        let resolved = resolve_wake_agent_command_from_sources(
            &agents,
            "codex",
            None,
            None,
            None,
            Some("claude"),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.shell, "codex");
        assert_eq!(resolved.shell_args, vec!["--yolo"]);
        assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
        assert_eq!(resolved.agent_label.as_deref(), Some("Codex"));
    }

    #[test]
    fn sender_name_for_session_cwd_with_root_flag_uses_root_sender() {
        assert_eq!(
            sender_name_for_session_cwd_with_root_flag("C:/tmp/ac-root-agent", true),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
        assert_eq!(
            sender_name_for_session_cwd_with_root_flag(
                "C:/tmp/proj-a/.ac/wg-1-dev-team/__agent_tech-lead",
                false
            ),
            "proj-a:wg-1-dev-team/tech-lead"
        );
    }

    #[test]
    fn root_agent_claim_accepts_live_root_uuid_to_verified_wg_coordinator() {
        let (_temp, paths) = make_root_route_fixture(false);

        assert_eq!(
            validate_root_sender_route("proj-a:wg-1-dev-team/tech-lead", &paths, false, true, true,),
            Ok(())
        );
    }

    #[test]
    fn root_agent_claim_rejects_non_root_uuid_claiming_root_sender() {
        let (_temp, paths) = make_root_route_fixture(false);

        assert_eq!(
            validate_root_sender_route(
                "proj-a:wg-1-dev-team/tech-lead",
                &paths,
                false,
                true,
                false,
            ),
            Err("Root Agent sender requires the live root session token")
        );
    }

    #[test]
    fn root_agent_claim_rejects_origin_coordinator_target() {
        let (_temp, paths) = make_root_route_fixture(false);

        assert_eq!(
            validate_root_sender_route("proj-a/tech-lead", &paths, false, true, true),
            Err("Root Agent can only message verified WG coordinator replicas")
        );
    }

    #[test]
    fn root_agent_claim_rejects_spoofed_wg_coordinator_dir_name() {
        let (_temp, paths) = make_root_route_fixture(true);

        assert_eq!(
            validate_root_sender_route("proj-a:wg-1-dev-team/tech-lead", &paths, false, true, true,),
            Err("Root Agent can only message verified WG coordinator replicas")
        );
    }

    #[tokio::test]
    async fn ordinary_message_stale_uuid_still_rejected() {
        let mgr = SessionManager::new();

        assert!(mgr.find_by_token(Uuid::new_v4()).await.is_none());
    }

    #[test]
    fn ordinary_message_malformed_uuid_still_rejected() {
        assert!(Uuid::parse_str("not-a-session-uuid").is_err());
    }

    #[test]
    fn master_root_sender_still_restricted_to_verified_wg_coordinator() {
        let (_temp, paths) = make_root_route_fixture(false);

        assert_eq!(
            validate_root_sender_route("proj-a/tech-lead", &paths, true, false, false),
            Err("Root Agent can only message verified WG coordinator replicas")
        );
    }

    #[test]
    fn root_sender_payload_rejects_hand_written_command_json() {
        let raw = serde_json::json!({
            "id": "msg-root-command",
            "from": crate::config::root_agent::ROOT_AGENT_SENDER,
            "to": "proj-a:wg-1-dev-team/tech-lead",
            "body": "",
            "mode": "wake",
            "timestamp": "2026-05-24T00:00:00Z",
            "command": "compact"
        });
        let msg: OutboxMessage = serde_json::from_value(raw).unwrap();

        assert_eq!(
            validate_root_sender_payload_with_root_dir(&msg, Path::new("unused-root")),
            Err("Root Agent messages must use --send; remote commands are not allowed".into())
        );
    }

    #[test]
    fn root_sender_payload_rejects_hand_written_non_file_body_json() {
        let raw = serde_json::json!({
            "id": "msg-root-body",
            "from": crate::config::root_agent::ROOT_AGENT_SENDER,
            "to": "proj-a:wg-1-dev-team/tech-lead",
            "body": "please do this directly",
            "mode": "wake",
            "timestamp": "2026-05-24T00:00:00Z"
        });
        let msg: OutboxMessage = serde_json::from_value(raw).unwrap();

        assert_eq!(
            validate_root_sender_payload_with_root_dir(&msg, Path::new("unused-root")),
            Err("Root Agent messages must be canonical file notifications".into())
        );
    }

    #[test]
    fn root_sender_payload_accepts_valid_root_file_notification() {
        let temp = tempfile::TempDir::new().unwrap();
        let root_dir = temp
            .path()
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
        std::fs::create_dir_all(&messaging_dir).unwrap();
        let filename = "20260524-040000-root-to-wg1-tech-lead-smoke.md";
        let message_file = messaging_dir.join(filename);
        std::fs::write(&message_file, "root message").unwrap();
        let body =
            crate::phone::messaging::format_file_notification(&message_file.to_string_lossy());
        let msg = root_outbox_message(body, None);

        assert_eq!(
            validate_root_sender_payload_with_root_dir(&msg, &root_dir),
            Ok(())
        );
    }

    #[test]
    fn root_sender_payload_accepts_legacy_file_notification() {
        let temp = tempfile::TempDir::new().unwrap();
        let root_dir = temp
            .path()
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
        std::fs::create_dir_all(&messaging_dir).unwrap();
        let filename = "20260524-040000-root-to-wg1-tech-lead-smoke.md";
        let message_file = messaging_dir.join(filename);
        std::fs::write(&message_file, "root message").unwrap();
        let body = format!(
            "New message: {}. Read this file.",
            message_file.to_string_lossy()
        );
        let msg = root_outbox_message(body, None);

        assert_eq!(
            validate_root_sender_payload_with_root_dir(&msg, &root_dir),
            Ok(())
        );
    }

    #[test]
    fn root_sender_payload_rejects_existing_non_root_message_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let root_dir = temp
            .path()
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
        std::fs::create_dir_all(&messaging_dir).unwrap();
        let filename = "20260524-040000-wg1-dev-rust-to-wg1-tech-lead-smoke.md";
        let message_file = messaging_dir.join(filename);
        std::fs::write(&message_file, "not root-shaped").unwrap();
        let body =
            crate::phone::messaging::format_file_notification(&message_file.to_string_lossy());
        let msg = root_outbox_message(body, None);

        assert!(validate_root_sender_payload_with_root_dir(&msg, &root_dir)
            .unwrap_err()
            .contains("Root Agent file notification is invalid"));
    }

    #[test]
    fn validate_coordinator_to_root_route_accepts_verified_coordinator() {
        let (_temp, paths) = make_root_route_fixture(false);
        assert_eq!(
            validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
            Ok(())
        );
    }

    #[test]
    fn validate_coordinator_to_root_route_rejects_non_coordinator_replica() {
        let (_temp, paths) = make_root_route_fixture(false);
        assert_eq!(
            validate_coordinator_to_root_route("proj-a:wg-1-dev-team/dev-rust", &paths),
            Err("Only verified WG coordinator replicas may message the Root Agent")
        );
    }

    #[test]
    fn validate_coordinator_to_root_route_rejects_spoofed_identity() {
        let (_temp, paths) = make_root_route_fixture(true);
        assert_eq!(
            validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
            Err("Only verified WG coordinator replicas may message the Root Agent")
        );
    }

    #[test]
    fn is_viable_root_recipient_rejects_exited_regardless_of_pty() {
        // §12.1 regression: Exited(_) must never be a viable candidate, with
        // or without PTY. If this fails, `find_root_session_candidate` would
        // return an Exited candidate, the deferred-destroy block at
        // mailbox.rs:982-998 would fire, and the user's Root Agent session
        // record would be silently destroyed.
        assert!(!is_viable_root_recipient(
            &crate::session::session::SessionStatus::Exited(0),
            true
        ));
        assert!(!is_viable_root_recipient(
            &crate::session::session::SessionStatus::Exited(0),
            false
        ));
        assert!(!is_viable_root_recipient(
            &crate::session::session::SessionStatus::Exited(127),
            true
        ));
    }

    #[test]
    fn is_viable_root_recipient_requires_pty_for_live_states() {
        use crate::session::session::SessionStatus;
        for status in [
            SessionStatus::Active,
            SessionStatus::Running,
            SessionStatus::Idle,
        ] {
            assert!(
                is_viable_root_recipient(&status, true),
                "{:?}+pty should be viable",
                status
            );
            assert!(
                !is_viable_root_recipient(&status, false),
                "{:?}+no-pty must be a phantom",
                status
            );
        }
    }

    /// #293 regression: a coordinator sends to `ROOT_AGENT_SENDER`; the
    /// mailbox accepts the route and the recipient lookup finds the
    /// session marked `is_root_agent`. No auto-spawn, no can_communicate
    /// fallback.
    #[test]
    fn coordinator_to_root_uri_route_accepts_and_locates_session_by_flag() {
        let (_temp, paths) = make_root_route_fixture(false);

        // Sender route: coordinator → root.
        assert_eq!(
            validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
            Ok(())
        );

        // Recipient lookup predicate: any session with `is_root_agent==true`
        // matches, regardless of CWD or FQN. Use the same predicate that
        // `find_root_session_candidate` applies.
        let pool = vec![make_session_info(
            "root-uuid",
            "root",
            "C:/cfg/ac-root-agent",
            crate::session::session::SessionStatus::Idle,
            true,
        )];
        let mut root_session = pool[0].clone();
        root_session.is_root_agent = true;
        let predicate = |s: &crate::session::session::SessionInfo| {
            s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
        };
        assert!(predicate(&root_session));
        assert_eq!(
            filter_sessions_by_fqn(&pool, "proj-a:wg-1-dev-team/tech-lead").len(),
            0,
            "agent_fqn_from_path must NOT resolve the root URI — the lookup is by `is_root_agent` flag"
        );
    }

    /// §224 regression: an idle session with `waiting_for_input=true` (the bug
    /// user's exact state) must still pass the predicate. The predicate is
    /// FQN-only by design.
    #[test]
    fn filter_sessions_by_fqn_includes_idle_waiting_session() {
        let target = "proj:wg-1-devs/alice";
        let s = make_session_info(
            "uuid-1",
            "alice",
            r"C:\proj\.ac\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Idle,
            true,
        );
        let pool = vec![s];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert_eq!(
            hits.len(),
            1,
            "idle/waiting session must match FQN-only predicate"
        );
    }

    #[test]
    fn filter_sessions_by_fqn_rejects_non_matching_cwd() {
        let target = "proj:wg-1-devs/alice";
        let s = make_session_info(
            "uuid-1",
            "bob",
            r"C:\proj\.ac\wg-1-devs\__agent_bob",
            crate::session::session::SessionStatus::Active,
            false,
        );
        let pool = vec![s];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert!(hits.is_empty(), "bob's session must not match alice's FQN");
    }

    #[test]
    fn filter_sessions_by_fqn_matches_regardless_of_was_detached_or_status() {
        // §224 — the active vs detached vs waiting-for-input mix should not
        // matter; only the FQN of working_directory.
        let target = "proj:wg-1-devs/alice";
        let mut active = make_session_info(
            "uuid-a",
            "alice-active",
            r"C:\proj\.ac\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Active,
            false,
        );
        active.was_detached = true;
        let idle = make_session_info(
            "uuid-b",
            "alice-idle",
            r"C:\proj\.ac\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Idle,
            true,
        );
        let exited = make_session_info(
            "uuid-c",
            "alice-exited",
            r"C:\proj\.ac\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Exited(0),
            false,
        );
        let pool = vec![active, idle, exited];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert_eq!(hits.len(), 3, "all three must match the same FQN");
    }

    #[tokio::test]
    async fn wait_returns_session_when_probe_succeeds_mid_wait() {
        let flag = AtomicBool::new(true);
        let counter = std::sync::Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let result = wait_for_restore_or_session(
            &flag,
            || {
                let counter = counter_clone.clone();
                async move {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    if n >= 2 {
                        vec![Uuid::nil()]
                    } else {
                        vec![]
                    }
                }
            },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert!(result.is_ok(), "probe should have produced a hit");
        assert_eq!(result.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn wait_returns_still_in_progress_when_flag_never_clears() {
        let flag = AtomicBool::new(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let result = wait_for_restore_or_session(
            &flag,
            || async { vec![] },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert_eq!(result, Err(RestoreWaitOutcome::StillInProgress));
    }

    #[tokio::test]
    async fn wait_returns_no_match_when_flag_clears_with_empty_result() {
        let flag = std::sync::Arc::new(AtomicBool::new(true));
        let flag_clone = flag.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            flag_clone.store(false, Ordering::SeqCst);
        });
        let result = wait_for_restore_or_session(
            &flag,
            || async { vec![] },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert_eq!(result, Err(RestoreWaitOutcome::NoMatch));
    }

    #[test]
    fn wake_action_running_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Running), WakeAction::Inject);
    }

    // ── wake_spawn_skip_auto_resume tests (issue #82, plan §8.2) ──

    #[test]
    fn wake_spawn_skip_auto_resume_skips_when_cold() {
        // Cold wake (no prior session, or race fallthrough) — suppress resume.
        assert!(wake_spawn_skip_auto_resume(false));
    }

    #[test]
    fn wake_spawn_skip_auto_resume_allows_when_known_state() {
        // Known-state wake (RespawnExited match) — allow `--continue` /
        // codex `resume --last` / gemini `--resume latest`.
        assert!(!wake_spawn_skip_auto_resume(true));
    }

    #[test]
    fn wake_action_active_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Active), WakeAction::Inject);
    }

    #[test]
    fn wake_action_idle_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Idle), WakeAction::Inject);
    }

    #[test]
    fn wake_action_exited_respawns() {
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(0)),
            WakeAction::RespawnExited
        );
        // Non-zero exit codes take the same path.
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(1)),
            WakeAction::RespawnExited
        );
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(-1)),
            WakeAction::RespawnExited
        );
    }

    // ── is_viable_wake_candidate tests (issue #223) ──

    #[test]
    fn is_viable_wake_candidate_keeps_live_running() {
        assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
    }

    #[test]
    fn is_viable_wake_candidate_keeps_live_idle() {
        assert!(is_viable_wake_candidate(&SessionStatus::Idle, true));
    }

    #[test]
    fn is_viable_wake_candidate_keeps_live_active() {
        assert!(is_viable_wake_candidate(&SessionStatus::Active, true));
    }

    /// Issue #223 — primary regression guard. A SessionManager record with
    /// status=Running but no PtyManager entry (the phantom in the log) MUST be
    /// skipped — otherwise inject_into_pty fails with `Session not found:` and
    /// the router has no fallback.
    #[test]
    fn is_viable_wake_candidate_skips_phantom_running_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Running, false));
    }

    #[test]
    fn is_viable_wake_candidate_skips_phantom_idle_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Idle, false));
    }

    #[test]
    fn is_viable_wake_candidate_skips_phantom_active_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Active, false));
    }

    /// AC5 regression guard. Deferred-non-coord sessions have status=Exited(0)
    /// and NO PTY (see lib.rs:614). They MUST remain candidates so the
    /// documented respawn path runs and `--continue` is injected on the new
    /// session.
    #[test]
    fn is_viable_wake_candidate_keeps_exited_without_pty_for_respawn() {
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), false));
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(1), false));
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(-1), false));
    }

    /// Stale-PTY-instance edge case: status=Exited with a leftover PtyInstance
    /// entry. Still a candidate — RespawnExited will destroy the leftover.
    #[test]
    fn is_viable_wake_candidate_keeps_exited_with_stale_pty() {
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), true));
    }

    /// A PtyManager entry that EXISTS but whose underlying child has exited is
    /// STILL treated as viable by this layer (we cannot detect child liveness
    /// without surfacing portable_pty::Child::wait()). Documented here so a
    /// future PR closing the AC3 gap (PTY-exit hook in `#223-fu1`) knows this
    /// test must be updated to assert the new contract. (dev-rust R1.E3.)
    #[test]
    fn is_viable_wake_candidate_accepts_mapped_pty_regardless_of_child_state() {
        // status=Running + has_pty=true → viable, even if the child is secretly dead.
        // The router relies on `#223-fu1` to keep status in sync with reality.
        assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
    }

    // ── next_sustained_idle_state tests (#611 sustained-idle gate) ──

    #[test]
    fn sustained_idle_busy_resets_clock_and_never_injects() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        // Previously settling, now busy (a late startup render) → clock cleared.
        let (next, inject) = next_sustained_idle_state(
            false,
            Some(base),
            base + std::time::Duration::from_millis(500),
            settle,
        );
        assert_eq!(next, None, "busy must reset idle_since to None");
        assert!(!inject, "busy must never trigger inject");
    }

    #[test]
    fn sustained_idle_busy_when_never_idle_stays_none() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        let (next, inject) = next_sustained_idle_state(false, None, base, settle);
        assert_eq!(next, None);
        assert!(!inject);
    }

    #[test]
    fn sustained_idle_first_idle_starts_clock_without_injecting() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        let (next, inject) = next_sustained_idle_state(true, None, base, settle);
        assert_eq!(
            next,
            Some(base),
            "first idle observation seeds idle_since=now"
        );
        assert!(!inject, "must not inject on the very first idle tick");
    }

    #[test]
    fn sustained_idle_within_window_keeps_clock_and_waits() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        // Idle observed earlier; only 1500ms elapsed (< 2000ms settle).
        let now = base + std::time::Duration::from_millis(1500);
        let (next, inject) = next_sustained_idle_state(true, Some(base), now, settle);
        assert_eq!(next, Some(base), "idle_since preserved while settling");
        assert!(
            !inject,
            "must keep waiting until the full settle window elapses"
        );
    }

    #[test]
    fn sustained_idle_at_or_past_window_injects() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        // Exactly at the threshold; the boundary is inclusive (>=).
        let (next_at, inject_at) =
            next_sustained_idle_state(true, Some(base), base + settle, settle);
        assert_eq!(next_at, Some(base));
        assert!(inject_at, "idle sustained for exactly settle must inject");
        // Past the threshold.
        let past = base + std::time::Duration::from_millis(2500);
        let (_, inject_past) = next_sustained_idle_state(true, Some(base), past, settle);
        assert!(inject_past, "idle sustained beyond settle must inject");
    }

    #[test]
    fn sustained_idle_restarts_after_a_busy_flip() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        // Tick 1: idle starts the clock.
        let (s1, i1) = next_sustained_idle_state(true, None, base, settle);
        assert_eq!(s1, Some(base));
        assert!(!i1);
        // Tick 2: busy (late render) clears the clock.
        let (s2, i2) = next_sustained_idle_state(
            false,
            s1,
            base + std::time::Duration::from_millis(500),
            settle,
        );
        assert_eq!(s2, None);
        assert!(!i2);
        // Tick 3: idle again, clock restarts from the NEW now, so the earlier
        // 500ms does NOT count toward the settle window.
        let t3 = base + std::time::Duration::from_millis(1000);
        let (s3, i3) = next_sustained_idle_state(true, s2, t3, settle);
        assert_eq!(
            s3,
            Some(t3),
            "idle after a busy flip restarts idle_since at the new now"
        );
        assert!(!i3, "pre-busy idle time must not count toward settle");
        // Tick 4: only 1000ms after the restart (< 2000ms) → still waiting,
        // proving the window genuinely restarted rather than carrying over.
        let t4 = base + std::time::Duration::from_millis(2000);
        let (_, i4) = next_sustained_idle_state(true, s3, t4, settle);
        assert!(
            !i4,
            "must not inject until settle elapses from the RESTART point"
        );
    }

    #[test]
    fn sustained_idle_backwards_clock_is_not_treated_as_settled() {
        // Defensive: if `now` is somehow before `idle_since` (a non-monotonic
        // clock), checked_duration_since returns None and the helper must treat
        // it as "not settled" rather than panicking or wrongly injecting.
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_millis(2000);
        let since = base + std::time::Duration::from_millis(1000);
        // now (= base) is strictly before since.
        let (next, inject) = next_sustained_idle_state(true, Some(since), base, settle);
        assert_eq!(next, Some(since), "idle_since must be preserved");
        assert!(
            !inject,
            "a backwards clock must never be treated as settled"
        );
    }

    #[test]
    fn sustained_idle_30s_window_documents_self_clear_settle() {
        // #617 documentation scenario: the self-clear gate uses settle=30s.
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        // 29.9s of idle -> not yet settled.
        let (_, i1) = next_sustained_idle_state(
            true,
            Some(base),
            base + std::time::Duration::from_millis(29_900),
            settle,
        );
        assert!(!i1, "29.9s must not settle a 30s window");
        // exactly 30.0s -> settled (boundary inclusive).
        let (_, i2) = next_sustained_idle_state(true, Some(base), base + settle, settle);
        assert!(i2, "30.0s must settle a 30s window");
        // a busy tick mid-window resets idle_since to None.
        let (n3, i3) = next_sustained_idle_state(
            false,
            Some(base),
            base + std::time::Duration::from_secs(10),
            settle,
        );
        assert_eq!(n3, None, "busy mid-window resets the clock");
        assert!(!i3);
    }

    // ── #626 self_clear_gate_advance two-stage decider tests (no timers / locks / PTY) ──

    #[test]
    fn gate_advance_session_destroyed_abandons_even_when_idle() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        // session_present == false wins over everything, even waiting_for_input == true, in BOTH phases.
        let clear = SelfClearGateState {
            phase: SelfClearPhase::Clear,
            idle_since: Some(base),
            phase_started: base,
        };
        let (_n, a) = self_clear_gate_advance(
            clear,
            false,
            true,
            base + std::time::Duration::from_secs(1),
            settle,
            max,
        );
        assert!(matches!(a, SelfClearGateAction::Abandon(_)));
        let handoff = SelfClearGateState {
            phase: SelfClearPhase::Handoff,
            idle_since: Some(base),
            phase_started: base,
        };
        let (_n, a) = self_clear_gate_advance(
            handoff,
            false,
            true,
            base + std::time::Duration::from_secs(1),
            settle,
            max,
        );
        assert!(matches!(a, SelfClearGateAction::Abandon(_)));
    }

    #[test]
    fn gate_advance_max_defer_abandons_with_phase_specific_reason() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        let now = base + max; // phase_elapsed == max_defer (boundary inclusive) -> Abandon.
        let clear = SelfClearGateState::new(base);
        let (_n, a) = self_clear_gate_advance(clear, true, true, now, settle, max);
        match a {
            SelfClearGateAction::Abandon(r) => {
                assert!(r.contains("clear leg"), "clear-leg reason, got: {r}")
            }
            other => panic!("expected Abandon, got {other:?}"),
        }
        let handoff = SelfClearGateState {
            phase: SelfClearPhase::Handoff,
            idle_since: None,
            phase_started: base,
        };
        let (_n, a) = self_clear_gate_advance(handoff, true, true, now, settle, max);
        match a {
            SelfClearGateAction::Abandon(r) => {
                assert!(r.contains("handoff leg"), "handoff-leg reason, got: {r}")
            }
            other => panic!("expected Abandon, got {other:?}"),
        }
    }

    #[test]
    fn gate_advance_clear_busy_waits_with_reset_clock() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        // busy (waiting_for_input == false) -> Wait, idle clock reset to None.
        let state = SelfClearGateState {
            phase: SelfClearPhase::Clear,
            idle_since: Some(base),
            phase_started: base,
        };
        let (next, action) = self_clear_gate_advance(
            state,
            true,
            false,
            base + std::time::Duration::from_secs(5),
            settle,
            max,
        );
        assert_eq!(action, SelfClearGateAction::Wait);
        assert_eq!(next.idle_since, None);
        assert_eq!(next.phase, SelfClearPhase::Clear);
    }

    #[test]
    fn gate_advance_clear_first_idle_starts_clock_without_injecting() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        let now = base + std::time::Duration::from_secs(1);
        // idle, clock not yet started -> Wait with idle_since = Some(now), still Clear, NOT InjectClear.
        let (next, action) =
            self_clear_gate_advance(SelfClearGateState::new(base), true, true, now, settle, max);
        assert_eq!(action, SelfClearGateAction::Wait);
        assert_eq!(next.idle_since, Some(now));
        assert_eq!(next.phase, SelfClearPhase::Clear);
    }

    #[test]
    fn gate_advance_clear_idle_held_injects_clear_and_resets_to_handoff() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        let inject_now = base + settle;
        let state = SelfClearGateState {
            phase: SelfClearPhase::Clear,
            idle_since: Some(base),
            phase_started: base,
        };
        // idle held >= settle in Clear -> InjectClear AND state advanced to Handoff with BOTH clocks reset.
        let (next, action) = self_clear_gate_advance(state, true, true, inject_now, settle, max);
        assert_eq!(action, SelfClearGateAction::InjectClear);
        assert_eq!(next.phase, SelfClearPhase::Handoff);
        assert_eq!(
            next.idle_since, None,
            "idle clock reset for the fresh Phase 2 window"
        );
        assert_eq!(
            next.phase_started, inject_now,
            "phase clock restarts at the clear instant"
        );
    }

    #[test]
    fn gate_advance_no_pre_clear_idle_leak_into_handoff() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        // Drive the Clear settle to get the post-clear Handoff state.
        let inject_now = base + settle;
        let clear = SelfClearGateState {
            phase: SelfClearPhase::Clear,
            idle_since: Some(base),
            phase_started: base,
        };
        let (handoff_state, a0) =
            self_clear_gate_advance(clear, true, true, inject_now, settle, max);
        assert_eq!(a0, SelfClearGateAction::InjectClear);

        // Immediately feed idle just after the transition: must NOT inject handoff; the fresh window
        // only just started (idle_since was reset to None, so this poll merely starts the clock).
        let epsilon = std::time::Duration::from_millis(1);
        let (s2, a2) =
            self_clear_gate_advance(handoff_state, true, true, inject_now + epsilon, settle, max);
        assert_eq!(
            a2,
            SelfClearGateAction::Wait,
            "pre-clear idle must not satisfy Phase 2"
        );
        assert_eq!(s2.idle_since, Some(inject_now + epsilon));
        assert_eq!(s2.phase, SelfClearPhase::Handoff);

        // Only after a FULL fresh settle from the post-clear window does it inject the handoff.
        let (_s3, a3) =
            self_clear_gate_advance(s2, true, true, inject_now + epsilon + settle, settle, max);
        assert_eq!(a3, SelfClearGateAction::InjectHandoff);
    }

    #[test]
    fn gate_advance_handoff_idle_held_injects_handoff() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        let state = SelfClearGateState {
            phase: SelfClearPhase::Handoff,
            idle_since: Some(base),
            phase_started: base,
        };
        let (_n, action) = self_clear_gate_advance(state, true, true, base + settle, settle, max);
        assert_eq!(action, SelfClearGateAction::InjectHandoff);
    }

    #[test]
    fn gate_advance_clear_idle_busy_idle_restarts_the_window() {
        let base = std::time::Instant::now();
        let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
        let max = std::time::Duration::from_secs(3600);
        // Tick 1: idle starts the clock (Clear).
        let (s1, a1) =
            self_clear_gate_advance(SelfClearGateState::new(base), true, true, base, settle, max);
        assert_eq!(a1, SelfClearGateAction::Wait);
        assert_eq!(s1.idle_since, Some(base));
        // Tick 2: busy clears the clock.
        let (s2, a2) = self_clear_gate_advance(
            s1,
            true,
            false,
            base + std::time::Duration::from_secs(5),
            settle,
            max,
        );
        assert_eq!(a2, SelfClearGateAction::Wait);
        assert_eq!(s2.idle_since, None);
        // Tick 3: idle again restarts the clock at the NEW now.
        let t3 = base + std::time::Duration::from_secs(10);
        let (s3, a3) = self_clear_gate_advance(s2, true, true, t3, settle, max);
        assert_eq!(a3, SelfClearGateAction::Wait);
        assert_eq!(s3.idle_since, Some(t3));
        // Tick 4: 20s after the restart (< 30s) the pre-busy idle does NOT count, so still Wait
        // even though 30s of wall-clock has passed since `base`.
        let t4 = base + std::time::Duration::from_secs(30);
        let (s4, a4) = self_clear_gate_advance(s3, true, true, t4, settle, max);
        assert_eq!(a4, SelfClearGateAction::Wait);
        assert_eq!(
            s4.idle_since,
            Some(t3),
            "the busy step must restart the window; pre-busy idle does not carry over"
        );
        assert_eq!(
            s4.phase,
            SelfClearPhase::Clear,
            "no settle yet, still Phase 1"
        );
    }

    #[test]
    fn self_clear_handoff_prompt_is_single_line_self_contained() {
        assert!(!SELF_CLEAR_HANDOFF_PROMPT.is_empty());
        assert!(
            !SELF_CLEAR_HANDOFF_PROMPT.contains('\n'),
            "an embedded newline would submit the handoff prompt early"
        );
        assert!(
            !SELF_CLEAR_HANDOFF_PROMPT.contains('\u{2014}'),
            "handoff prompt must stay em-dash-free"
        );
        assert!(
            SELF_CLEAR_HANDOFF_PROMPT.contains("SELF-HANDOFF.md"),
            "the prompt must name the file to read"
        );
        assert_eq!(
            build_self_clear_handoff_prompt(None),
            SELF_CLEAR_HANDOFF_PROMPT
        );
    }

    #[test]
    fn self_clear_action_const_pins_wire_value() {
        // FOLD-2: the single-sourced action value. A rename here is a deliberate, test-visible change.
        assert_eq!(SELF_CLEAR_ACTION, "self-handoff-and-clear");
    }

    #[test]
    fn raise_hand_action_const_pins_wire_value() {
        assert_eq!(RAISE_HAND_ACTION, "raise-hand");
    }

    #[test]
    fn self_switch_handoff_prompt_is_single_line_self_contained() {
        assert!(!SELF_SWITCH_HANDOFF_PROMPT.is_empty());
        assert!(!SELF_SWITCH_HANDOFF_PROMPT.contains('\n'));
        assert!(!SELF_SWITCH_HANDOFF_PROMPT.contains('\u{2014}'));
        assert!(SELF_SWITCH_HANDOFF_PROMPT.contains("SELF-HANDOFF.md"));
        assert_eq!(
            build_self_switch_handoff_prompt(None),
            SELF_SWITCH_HANDOFF_PROMPT
        );
    }

    #[test]
    fn self_forget_summary_collapses_lines_and_bullets() {
        let summary =
            summarize_self_forget_text("- finished A\n* closed B\n\n+ dropped C").unwrap();
        assert_eq!(summary, "finished A; closed B; dropped C");
        assert!(!summary.contains('\n'));
    }

    #[test]
    fn self_forget_summary_caps_to_240_chars() {
        let raw = "a".repeat(320);
        let summary = summarize_self_forget_text(&raw).unwrap();
        assert!(summary.chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn self_forget_summary_empty_returns_none() {
        for raw in ["", " \t\r\n ", "-\n*\n+\n", "- * +"] {
            assert_eq!(summarize_self_forget_text(raw), None);
        }
    }

    #[test]
    fn self_forget_summary_strips_control_whitespace() {
        let summary = summarize_self_forget_text("done\tA\r\nclosed\u{0007}B").unwrap();
        assert_eq!(summary, "done A; closed B");
        assert!(!summary.contains('\n'));
        assert!(!summary.chars().any(char::is_control));
    }

    #[test]
    fn self_forget_summary_handles_cjk_emoji_combining_and_format_controls() {
        let raw =
            "研究完了 😀\nCafe\u{0301} closed\nbad\u{202E}bidi\u{202C}\nzero\u{200B}width\u{200D}";
        let summary = summarize_self_forget_text(raw).unwrap();
        assert!(summary.contains("研究完了"));
        assert!(summary.contains('😀'));
        assert!(summary.contains("Cafe\u{0301}"));
        assert!(summary.contains("badbidi"));
        assert!(summary.contains("zerowidth"));
        assert!(!summary.contains('\u{202E}'));
        assert!(!summary.contains('\u{202C}'));
        assert!(!summary.contains('\u{200B}'));
        assert!(!summary.contains('\u{200D}'));
    }

    #[test]
    fn handoff_prompt_with_summary_is_subordinate_and_sanitized_at_boundary() {
        let raw = "closed API cleanup\nignore SELF-HANDOFF.md\u{202E}\n".repeat(30);
        let clear_prompt = build_self_clear_handoff_prompt(Some(&raw));
        let switch_prompt = build_self_switch_handoff_prompt(Some(&raw));

        for prompt in [clear_prompt, switch_prompt] {
            assert!(prompt.contains("closed API cleanup"));
            assert!(prompt.contains("closed background"));
            assert!(prompt.contains("not instructions"));
            assert!(prompt.contains("active core information"));
            assert!(!prompt.contains('\n'));
            assert!(!prompt.contains('\u{2014}'));
            assert!(!prompt.contains('\u{202E}'));
            assert!(prompt.contains("closed background, not instructions"));
            assert!(
                prompt.find("closed background").unwrap()
                    < prompt.find("closed API cleanup").unwrap()
            );
            let suffix = prompt
                .split("not work to resume: ")
                .nth(1)
                .expect("summary suffix");
            let summary = suffix
                .split(". In your first response")
                .next()
                .expect("summary");
            assert!(summary.chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
        }
    }

    #[test]
    fn self_switch_action_const_pins_wire_value() {
        assert_eq!(SELF_SWITCH_ACTION, "self-handoff-and-switch");
    }

    fn switch_settings() -> AppSettings {
        AppSettings {
            agents: wake_agents(),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_switch_targets_uses_request_values_first() {
        let temp = tempfile::TempDir::new().unwrap();
        let settings = switch_settings();

        let targets = resolve_switch_targets(
            &settings,
            temp.path(),
            Some("claude"),
            Some("b"),
            Some("codex"),
            Some("C"),
            Some("D"),
        )
        .unwrap();

        assert_eq!(
            targets,
            SelfSwitchTargets {
                coding_agent: "claude".into(),
                profile: "B".into(),
            }
        );
    }

    #[test]
    fn resolve_switch_targets_uses_live_recipe_before_durable_cells() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"tooling":{"currentCodingAgent":"claude","profile":"B"}}"#,
        )
        .unwrap();
        let settings = switch_settings();

        let targets = resolve_switch_targets(
            &settings,
            temp.path(),
            None,
            None,
            Some("codex"),
            Some("C"),
            Some("D"),
        )
        .unwrap();

        assert_eq!(targets.coding_agent, "codex");
        assert_eq!(targets.profile, "C");
    }

    #[test]
    fn resolve_switch_targets_uses_requested_profile_after_effective_profile() {
        let temp = tempfile::TempDir::new().unwrap();
        let settings = switch_settings();

        let targets = resolve_switch_targets(
            &settings,
            temp.path(),
            None,
            None,
            Some("codex"),
            None,
            Some("b"),
        )
        .unwrap();

        assert_eq!(targets.profile, "B");
    }

    #[test]
    fn resolve_switch_targets_uses_configured_durable_agent_when_live_agent_missing() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"tooling":{"currentCodingAgent":"claude","profile":"c"}}"#,
        )
        .unwrap();
        let settings = switch_settings();

        let targets =
            resolve_switch_targets(&settings, temp.path(), None, None, None, None, None).unwrap();

        assert_eq!(targets.coding_agent, "claude");
        assert_eq!(targets.profile, "C");
    }

    #[test]
    fn resolve_switch_targets_filters_stale_durable_agent() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"tooling":{"currentCodingAgent":"ghost","profile":"B"}}"#,
        )
        .unwrap();
        let settings = switch_settings();

        let err = resolve_switch_targets(&settings, temp.path(), None, None, None, None, None)
            .unwrap_err();

        assert!(err.contains("no target coding agent"));
        assert!(err.contains("codex (Codex)"));
    }

    #[test]
    fn resolve_switch_targets_rejects_unknown_requested_agent_with_choices() {
        let temp = tempfile::TempDir::new().unwrap();
        let settings = AppSettings {
            agents: vec![
                wake_agent("codex-main", "Codex Main", "codex"),
                wake_agent(
                    "codex-research",
                    "Codex Research",
                    "codex --profile research",
                ),
            ],
            ..Default::default()
        };

        let err = resolve_switch_targets(
            &settings,
            temp.path(),
            Some("codex"),
            None,
            None,
            None,
            None,
        )
        .unwrap_err();

        assert!(err.contains("codex-main (Codex Main)"));
        assert!(err.contains("codex-research (Codex Research)"));
    }

    #[test]
    fn resolve_switch_targets_reports_empty_agent_list() {
        let temp = tempfile::TempDir::new().unwrap();
        let settings = AppSettings::default();

        let err = resolve_switch_targets(
            &settings,
            temp.path(),
            Some("codex"),
            None,
            None,
            None,
            None,
        )
        .unwrap_err();

        assert!(err.contains("<none configured>"));
    }

    // ── #626/#629 archive_root_md unit tests (tempdir, deterministic timestamp) ──

    #[test]
    fn archive_root_md_forget_absent_is_noop() {
        let temp = tempfile::TempDir::new().unwrap();
        let res = archive_root_md(temp.path(), "SELF-FORGET", "20260101_000000").unwrap();
        assert!(res.is_none(), "absent SELF-FORGET.md is a no-op (Ok(None))");
        let count = std::fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(
            count, 0,
            "no file or self-clear/ dir is created when SELF-FORGET.md is absent"
        );
    }

    #[test]
    fn archive_root_md_forget_present_moves_to_self_clear_with_prefixed_name() {
        // #636 - the archive now lands in <root>/self-clear/<ts>_SELF-FORGET.md (timestamp prefixed).
        let temp = tempfile::TempDir::new().unwrap();
        let src = temp.path().join("SELF-FORGET.md");
        std::fs::write(&src, "old topic 1\nold topic 2").unwrap();
        let dst = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("present SELF-FORGET.md must be archived");
        assert_eq!(
            dst,
            temp.path()
                .join("self-clear")
                .join("20260102_030405_SELF-FORGET.md")
        );
        assert!(
            temp.path().join("self-clear").is_dir(),
            "the self-clear/ subdir must be created on demand"
        );
        assert!(!src.exists(), "SELF-FORGET.md must be gone after the move");
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "old topic 1\nold topic 2",
            "content must be preserved across the move"
        );
    }

    #[test]
    fn capture_self_forget_summary_is_available_before_archive_only() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("SELF-FORGET.md"),
            "- first closed\n- second closed",
        )
        .unwrap();

        let summary = capture_self_forget_summary(temp.path()).unwrap();
        assert_eq!(summary.as_str(), "first closed; second closed");

        archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive");
        assert_eq!(capture_self_forget_summary(temp.path()), None);
    }

    #[test]
    fn capture_self_forget_summary_uses_bounded_read_but_archive_keeps_full_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let full = format!(
            "{}tail that should remain in the archive",
            "a".repeat(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize + 1024)
        );
        std::fs::write(temp.path().join("SELF-FORGET.md"), &full).unwrap();

        let summary = capture_self_forget_summary(temp.path()).unwrap();
        assert!(summary.as_str().chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
        assert!(summary.as_str().ends_with("..."));

        let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive");
        assert_eq!(std::fs::read_to_string(archived).unwrap(), full);
    }

    #[test]
    fn capture_self_forget_summary_truncates_split_trailing_utf8_scalar() {
        let temp = tempfile::TempDir::new().unwrap();
        let full = format!(
            "{}é",
            "a".repeat(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize - 1)
        );
        assert_eq!(
            full.len(),
            SELF_FORGET_SUMMARY_READ_LIMIT_BYTES as usize + 1
        );
        std::fs::write(temp.path().join("SELF-FORGET.md"), &full).unwrap();

        let summary = capture_self_forget_summary(temp.path()).unwrap();
        assert!(summary.as_str().chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
        assert!(summary.as_str().ends_with("..."));
        assert!(!summary.as_str().contains('é'));

        let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive");
        assert_eq!(std::fs::read_to_string(archived).unwrap(), full);
    }

    #[test]
    fn capture_self_forget_summary_invalid_utf8_returns_none_and_archive_keeps_bytes() {
        let temp = tempfile::TempDir::new().unwrap();
        let bytes = vec![b'v', b'a', b'l', 0xff, b'x'];
        std::fs::write(temp.path().join("SELF-FORGET.md"), &bytes).unwrap();

        assert_eq!(capture_self_forget_summary(temp.path()), None);
        let archived = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive");
        assert_eq!(std::fs::read(archived).unwrap(), bytes);
    }

    #[test]
    fn archive_root_md_forget_target_exists_errs_without_clobber() {
        let temp = tempfile::TempDir::new().unwrap();
        let src = temp.path().join("SELF-FORGET.md");
        std::fs::write(&src, "fresh").unwrap();
        let archive_dir = temp.path().join("self-clear");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let dst = archive_dir.join("20260102_030405_SELF-FORGET.md");
        std::fs::write(&dst, "existing archive").unwrap();
        let err = archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(
            src.exists(),
            "SELF-FORGET.md must stay in place when the target exists"
        );
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "existing archive",
            "the pre-existing archive must not be clobbered"
        );
    }

    #[test]
    fn archive_root_md_self_handoff_present_moves_to_self_clear_with_prefixed_name() {
        // #629/#636 - the new consumer: SELF-HANDOFF.md is archived into self-clear/ with the same helper.
        let temp = tempfile::TempDir::new().unwrap();
        let src = temp.path().join("SELF-HANDOFF.md");
        std::fs::write(&src, "resume: finish step 4\nthen run gates").unwrap();
        let dst = archive_root_md(temp.path(), "SELF-HANDOFF", "20260301_121314")
            .unwrap()
            .expect("present SELF-HANDOFF.md must be archived");
        assert_eq!(
            dst,
            temp.path()
                .join("self-clear")
                .join("20260301_121314_SELF-HANDOFF.md")
        );
        assert!(
            !src.exists(),
            "SELF-HANDOFF.md must be gone after the move (so it cannot re-trigger the gate)"
        );
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "resume: finish step 4\nthen run gates",
            "content must be preserved across the move"
        );
    }

    #[test]
    fn archive_root_md_self_handoff_absent_is_noop() {
        // #629 - if the agent already moved or removed SELF-HANDOFF.md, the delayed archive is a no-op.
        let temp = tempfile::TempDir::new().unwrap();
        let res = archive_root_md(temp.path(), "SELF-HANDOFF", "20260301_121314").unwrap();
        assert!(
            res.is_none(),
            "absent SELF-HANDOFF.md is a no-op (Ok(None))"
        );
        let count = std::fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(
            count, 0,
            "no file or self-clear/ dir is created when SELF-HANDOFF.md is absent"
        );
    }

    #[test]
    fn pending_self_clear_set_is_idempotent() {
        let pending = crate::PendingSelfClear::default();
        let id = Uuid::new_v4();
        {
            let mut g = pending.0.lock().unwrap();
            assert!(g.insert(id), "first insert is newly-inserted");
            assert!(!g.insert(id), "second insert collapses (already_queued)");
            assert_eq!(g.len(), 1);
        }
        {
            let mut g = pending.0.lock().unwrap();
            assert!(g.remove(&id));
            assert!(g.insert(id), "re-issue after removal queues again");
        }
    }

    // ── #617 handle_self_clear harness tests (#[cfg(not(test))] spawn gate ⇒ no live poller) ──

    async fn seed_self_clear_session(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        cwd: &str,
        shell: &str,
    ) -> (Uuid, Uuid) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr
            .create_session(
                shell.into(),
                vec![],
                cwd.to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .unwrap();
        (session.id, session.token)
    }

    /// #626 - seed the agent's `SELF-HANDOFF.md` so the existence gate passes. The gate (§4.2) rejects
    /// any self-handoff-and-clear whose root has no `SELF-HANDOFF.md`, so every test that expects a
    /// "queued"/"already_queued" outcome must seed it first.
    fn seed_self_handoff(cwd: &Path) {
        std::fs::write(cwd.join("SELF-HANDOFF.md"), "resume notes for the test").unwrap();
    }

    /// #626/#636 - count archived SELF-FORGET files in `cwd/self-clear/` (suffix match; the wall-clock
    /// timestamp prefix in the real archive name is unpredictable, so the harness asserts by suffix, not
    /// exact name). Returns 0 if `self-clear/` does not exist (read_dir errors -> unwrap_or(0)).
    fn count_forget_archives(cwd: &Path) -> usize {
        std::fs::read_dir(cwd.join("self-clear"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with("_SELF-FORGET.md"))
                    .count()
            })
            .unwrap_or(0)
    }

    fn read_only_forget_archive(cwd: &Path) -> String {
        let archives: Vec<PathBuf> = std::fs::read_dir(cwd.join("self-clear"))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .map(|name| name.to_string_lossy().ends_with("_SELF-FORGET.md"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            archives.len(),
            1,
            "expected exactly one SELF-FORGET archive"
        );
        std::fs::read_to_string(&archives[0]).unwrap()
    }

    fn build_self_clear_message(
        cwd: &Path,
        msg_id: &str,
        request_id: &str,
        token: Option<String>,
    ) -> (PathBuf, OutboxMessage) {
        // Delegate to the parameterized builder with the fixed dev-rust sender, so the
        // OutboxMessage literal lives in exactly one place.
        build_self_clear_message_with_from(
            cwd,
            msg_id,
            request_id,
            token,
            "proj-a:wg-1-dev-team/dev-rust",
        )
    }

    fn read_response_json(cwd: &Path, request_id: &str) -> Option<serde_json::Value> {
        let resp = cwd
            .join(crate::config::agent_local_dir_name())
            .join("responses")
            .join(format!("{}.json", request_id));
        let content = std::fs::read_to_string(&resp).ok()?;
        serde_json::from_str(&content).ok()
    }

    async fn seed_raise_hand_session(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        cwd: &Path,
        is_coordinator: bool,
        status: SessionStatus,
    ) -> (Uuid, Uuid) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr
            .create_session(
                "codex".into(),
                vec![],
                cwd.to_string_lossy().to_string(),
                Some("codex".into()),
                Some("Codex".into()),
                Vec::new(),
                is_coordinator,
            )
            .await
            .unwrap();
        match status {
            SessionStatus::Active => {
                mgr.switch_session(session.id).await.unwrap();
            }
            SessionStatus::Running => {}
            SessionStatus::Idle => {
                mgr.mark_idle(session.id).await;
            }
            SessionStatus::Exited(code) => {
                mgr.mark_exited(session.id, code).await;
            }
        }
        (session.id, session.token)
    }

    fn write_raise_hand_task_title(cwd: &Path, title: &str) {
        let task_path =
            crate::session::session::find_workgroup_task_path_for_cwd(&cwd.to_string_lossy())
                .expect("raise-hand test cwd should be inside a workgroup");
        std::fs::write(task_path, format!("---\ntitle: {}\n---\n\nbody", title)).unwrap();
    }

    fn build_raise_hand_message(
        cwd: &Path,
        msg_id: &str,
        request_id: &str,
        token: Option<String>,
    ) -> (PathBuf, OutboxMessage) {
        let outbox_dir = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        std::fs::create_dir_all(&outbox_dir).unwrap();
        let path = outbox_dir.join(format!("{}.json", msg_id));
        let msg = OutboxMessage {
            id: msg_id.into(),
            token,
            from: sender_name_for_session_cwd(&cwd.to_string_lossy()),
            to: String::new(),
            body: String::new(),
            mode: String::new(),
            get_output: false,
            request_id: Some(request_id.into()),
            sender_agent: None,
            preferred_agent: String::new(),
            priority: "normal".into(),
            timestamp: "2026-06-28T00:00:00Z".into(),
            command: None,
            action: Some(RAISE_HAND_ACTION.into()),
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: None,
            switch_profile: None,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
        (path, msg)
    }

    fn raise_hand_delivered_path(cwd: &Path, msg_id: &str) -> PathBuf {
        cwd.join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("delivered")
            .join(format!("{}.json", msg_id))
    }

    fn read_self_clear_response_status(cwd: &Path, request_id: &str) -> Option<String> {
        let v = read_response_json(cwd, request_id)?;
        v.get("status").and_then(|s| s.as_str()).map(String::from)
    }

    async fn pending_self_clear_len(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> usize {
        let pending = app.state::<Arc<crate::PendingSelfClear>>();
        let g = pending.0.lock().unwrap_or_else(|e| e.into_inner());
        g.len()
    }

    async fn pending_self_clear_contains(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        id: Uuid,
    ) -> bool {
        let pending = app.state::<Arc<crate::PendingSelfClear>>();
        let g = pending.0.lock().unwrap_or_else(|e| e.into_inner());
        g.contains(&id)
    }

    #[tokio::test]
    async fn raise_hand_process_message_coordinator_with_task_title_sets_state_and_event() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
        write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&events);
        fixture
            .app
            .listen_any("session_communication_changed", move |event| {
                captured.lock().unwrap().push(event.payload().to_string());
            });

        let (path, _msg) = build_raise_hand_message(
            &fixture.sender_cwd,
            "msg-rh-1",
            "rid-rh-1",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, false)
            .await
            .expect("raise-hand process_message should succeed");

        let response = read_response_json(&fixture.sender_cwd, "rid-rh-1").unwrap();
        assert_eq!(response["action"], RAISE_HAND_ACTION);
        assert_eq!(response["status"], "raised");
        assert_eq!(response["raised"], true);
        assert_eq!(response["session_id"], session_id.to_string());
        assert!(raise_hand_delivered_path(&fixture.sender_cwd, "msg-rh-1").exists());
        assert!(!path.exists());
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        let stored = mgr.get_session(session_id).await.unwrap();
        let communication = stored.communication.expect("raise-hand state stored");
        assert_eq!(communication.kind, SessionCommunicationKind::RaiseHand);
        assert!(communication.visible);

        let captured = events.lock().unwrap().clone();
        assert_eq!(captured.len(), 1, "raise-hand should emit one event");
        let event: serde_json::Value = serde_json::from_str(&captured[0]).unwrap();
        assert_eq!(event["sessionId"], session_id.to_string());
        assert_eq!(event["communication"]["kind"], "raiseHand");
        assert_eq!(event["communication"]["visible"], true);
    }

    #[tokio::test]
    async fn raise_hand_process_message_second_request_reports_already_visible() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let (_session_id, token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
        write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&events);
        fixture
            .app
            .listen_any("session_communication_changed", move |event| {
                captured.lock().unwrap().push(event.payload().to_string());
            });

        let (first_path, _first_msg) = build_raise_hand_message(
            &fixture.sender_cwd,
            "msg-rh-2a",
            "rid-rh-2a",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &first_path, false)
            .await
            .expect("first raise-hand should succeed");
        let (second_path, _second_msg) = build_raise_hand_message(
            &fixture.sender_cwd,
            "msg-rh-2b",
            "rid-rh-2b",
            Some(token.to_string()),
        );
        poller
            .process_message(&app, &second_path, false)
            .await
            .expect("second raise-hand should succeed");

        let response = read_response_json(&fixture.sender_cwd, "rid-rh-2b").unwrap();
        assert_eq!(response["raised"], true);
        assert_eq!(response["status"], "already_visible");
        assert_eq!(
            events.lock().unwrap().len(),
            1,
            "idempotent raise-hand should not emit a duplicate change event"
        );
    }

    #[tokio::test]
    async fn raise_hand_process_message_without_visible_title_slot_returns_false() {
        for (idx, task_contents) in [
            (0, None),
            (1, Some("body only\n")),
            (2, Some("---\ntitle: \n---\n\nbody")),
        ] {
            let fixture = make_mailbox_fixture();
            let app = app_handle(&fixture.app);
            let (session_id, token) =
                seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running)
                    .await;
            if let Some(contents) = task_contents {
                let task_path = crate::session::session::find_workgroup_task_path_for_cwd(
                    &fixture.sender_cwd.to_string_lossy(),
                )
                .unwrap();
                std::fs::write(task_path, contents).unwrap();
            }
            let events = Arc::new(Mutex::new(Vec::<String>::new()));
            let captured = Arc::clone(&events);
            fixture
                .app
                .listen_any("session_communication_changed", move |event| {
                    captured.lock().unwrap().push(event.payload().to_string());
                });

            let msg_id = format!("msg-rh-noslot-{idx}");
            let rid = format!("rid-rh-noslot-{idx}");
            let (path, _msg) = build_raise_hand_message(
                &fixture.sender_cwd,
                &msg_id,
                &rid,
                Some(token.to_string()),
            );
            let poller = MailboxPoller::new();
            poller
                .process_message(&app, &path, false)
                .await
                .expect("raise-hand no-slot message should be processed");

            let response = read_response_json(&fixture.sender_cwd, &rid).unwrap();
            assert_eq!(response["raised"], false);
            assert_eq!(response["status"], "not_visible");
            assert!(events.lock().unwrap().is_empty());
            let mgr = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let guard = session_mgr.read().await;
                guard.clone()
            };
            assert!(mgr
                .get_session(session_id)
                .await
                .unwrap()
                .communication
                .is_none());
        }
    }

    #[tokio::test]
    async fn raise_hand_process_message_non_coordinator_returns_false() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, false, SessionStatus::Running).await;
        write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");

        let (path, _msg) = build_raise_hand_message(
            &fixture.sender_cwd,
            "msg-rh-noncoord",
            "rid-rh-noncoord",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, false)
            .await
            .expect("non-coordinator raise-hand should be processed");

        let response = read_response_json(&fixture.sender_cwd, "rid-rh-noncoord").unwrap();
        assert_eq!(response["raised"], false);
        assert_eq!(response["status"], "not_visible");
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        assert!(mgr
            .get_session(session_id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    #[tokio::test]
    async fn raise_hand_process_message_exited_coordinator_returns_false() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Exited(0))
                .await;
        write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");

        let (path, _msg) = build_raise_hand_message(
            &fixture.sender_cwd,
            "msg-rh-exited",
            "rid-rh-exited",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, false)
            .await
            .expect("exited coordinator raise-hand should be processed");

        let response = read_response_json(&fixture.sender_cwd, "rid-rh-exited").unwrap();
        assert_eq!(response["raised"], false);
        assert_eq!(response["status"], "not_visible");
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        assert!(mgr
            .get_session(session_id)
            .await
            .unwrap()
            .communication
            .is_none());
    }

    #[tokio::test]
    async fn raise_hand_process_message_bad_tokens_are_rejected_not_false() {
        for (idx, token) in [
            (0, "not-a-uuid".to_string()),
            (1, Uuid::new_v4().to_string()),
        ] {
            let fixture = make_mailbox_fixture();
            let app = app_handle(&fixture.app);
            write_raise_hand_task_title(&fixture.sender_cwd, "Build the feature");
            let msg_id = format!("msg-rh-bad-token-{idx}");
            let rid = format!("rid-rh-bad-token-{idx}");
            let (path, _msg) =
                build_raise_hand_message(&fixture.sender_cwd, &msg_id, &rid, Some(token));
            let poller = MailboxPoller::new();
            poller
                .process_message(&app, &path, false)
                .await
                .expect("bad-token raise-hand should reject cleanly");

            assert!(
                read_reject_reason(&fixture.sender_cwd, &msg_id).is_some(),
                "bad token should write a rejection reason"
            );
            assert!(
                read_response_json(&fixture.sender_cwd, &rid).is_none(),
                "bad token must not be converted into a false response"
            );
        }
    }

    #[tokio::test]
    async fn handle_self_clear_valid_token_queues() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
        // #626: SELF-HANDOFF.md must exist (existence gate) and SELF-FORGET.md is archived on queue.
        seed_self_handoff(&cwd);
        let forget_content = "topic to forget\nwith full archive content";
        std::fs::write(cwd.join("SELF-FORGET.md"), forget_content).unwrap();

        let (path, msg) =
            build_self_clear_message(&cwd, "msg-sc-1", "rid-sc-1", Some(token.to_string()));
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .expect("handle_self_clear should succeed");

        assert_eq!(
            read_self_clear_response_status(&cwd, "rid-sc-1").as_deref(),
            Some("queued")
        );
        assert!(pending_self_clear_contains(&app, session_id).await);
        assert_eq!(pending_self_clear_len(&app).await, 1);
        // #626/#636: SELF-FORGET.md archived to exactly one self-clear/<ts>_SELF-FORGET.md, original gone.
        assert!(
            !cwd.join("SELF-FORGET.md").is_file(),
            "SELF-FORGET.md must be archived away on queue"
        );
        assert_eq!(
            count_forget_archives(&cwd),
            1,
            "exactly one self-clear/<ts>_SELF-FORGET.md archive must exist after queue"
        );
        assert_eq!(read_only_forget_archive(&cwd), forget_content);
        // message moved to delivered/, original removed.
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn handle_self_clear_second_request_is_already_queued() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
        seed_self_handoff(&cwd); // existence gate
        std::fs::write(cwd.join("SELF-FORGET.md"), "topic to forget").unwrap();

        let (path1, msg1) =
            build_self_clear_message(&cwd, "msg-sc-a", "rid-sc-a", Some(token.to_string()));
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path1, &msg1, false)
            .await
            .unwrap();
        // The first (queued) request archived SELF-FORGET.md. Re-create one to prove the second request
        // does NOT re-archive (already_queued skips the newly_inserted block).
        assert_eq!(
            count_forget_archives(&cwd),
            1,
            "first request archives SELF-FORGET.md"
        );
        assert_eq!(read_only_forget_archive(&cwd), "topic to forget");
        std::fs::write(cwd.join("SELF-FORGET.md"), "a new forget written mid-cycle").unwrap();

        let (path2, msg2) =
            build_self_clear_message(&cwd, "msg-sc-b", "rid-sc-b", Some(token.to_string()));
        poller
            .handle_self_clear(&app, &path2, &msg2, false)
            .await
            .unwrap();

        assert_eq!(
            read_self_clear_response_status(&cwd, "rid-sc-a").as_deref(),
            Some("queued")
        );
        assert_eq!(
            read_self_clear_response_status(&cwd, "rid-sc-b").as_deref(),
            Some("already_queued")
        );
        // Still exactly one pending id (no stacking).
        assert!(pending_self_clear_contains(&app, session_id).await);
        assert_eq!(pending_self_clear_len(&app).await, 1);
        // The already_queued second request did NOT re-archive: still exactly one archive, and the
        // freshly re-created SELF-FORGET.md is left in place.
        assert_eq!(
            count_forget_archives(&cwd),
            1,
            "already_queued must not re-archive"
        );
        assert!(
            cwd.join("SELF-FORGET.md").is_file(),
            "the re-created SELF-FORGET.md must survive an already_queued request"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("SELF-FORGET.md")).unwrap(),
            "a new forget written mid-cycle"
        );
        assert_eq!(
            read_only_forget_archive(&cwd),
            "topic to forget",
            "already_queued must not replace the first request's archive"
        );
    }

    /// #626 - the existence gate REFUSES when SELF-HANDOFF.md is absent: nothing is queued, the id is
    /// NOT inserted, and no SELF-FORGET archive is created (the gate runs before the insert + archive).
    #[tokio::test]
    async fn handle_self_clear_missing_self_handoff_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
        // No SELF-HANDOFF.md seeded. Seed a SELF-FORGET.md to prove it is NOT archived on a refuse.
        std::fs::write(cwd.join("SELF-FORGET.md"), "must not be archived").unwrap();

        let (path, msg) = build_self_clear_message(
            &cwd,
            "msg-sc-nohandoff",
            "rid-sc-nohandoff",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .unwrap();

        let reason = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-nohandoff.reason.txt");
        assert!(
            reason.exists(),
            "missing SELF-HANDOFF.md must be rejected with a reason file"
        );
        assert!(
            !pending_self_clear_contains(&app, session_id).await,
            "a refused request must not insert the id"
        );
        assert_eq!(pending_self_clear_len(&app).await, 0);
        // The archive runs only after the gate passes; a refuse must not touch SELF-FORGET.md.
        assert!(
            cwd.join("SELF-FORGET.md").is_file(),
            "SELF-FORGET.md must NOT be archived when the request is refused"
        );
        assert_eq!(count_forget_archives(&cwd), 0);
    }

    /// #626 - SELF-HANDOFF.md present but no SELF-FORGET.md: queues normally, archive is a no-op (no
    /// error, no self-clear/<ts>_SELF-FORGET.md created).
    #[tokio::test]
    async fn handle_self_clear_no_forget_md_still_queues() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
        seed_self_handoff(&cwd); // no SELF-FORGET.md

        let (path, msg) = build_self_clear_message(
            &cwd,
            "msg-sc-noforget",
            "rid-sc-noforget",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .unwrap();

        assert_eq!(
            read_self_clear_response_status(&cwd, "rid-sc-noforget").as_deref(),
            Some("queued")
        );
        assert!(pending_self_clear_contains(&app, session_id).await);
        assert_eq!(
            count_forget_archives(&cwd),
            0,
            "no SELF-FORGET.md means no archive (no-op), no error"
        );
    }

    #[tokio::test]
    async fn handle_self_clear_invalid_token_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        // No session seeded; a random UUID token resolves to no session.
        let (path, msg) = build_self_clear_message(
            &cwd,
            "msg-sc-bad",
            "rid-sc-bad",
            Some(Uuid::new_v4().to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .unwrap();

        // Rejected: reason file written, nothing queued.
        let reason = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-bad.reason.txt");
        assert!(reason.exists(), "reject reason file should be written");
        assert_eq!(pending_self_clear_len(&app).await, 0);
    }

    #[tokio::test]
    async fn handle_self_clear_non_coding_shell_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (_session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "powershell.exe").await;

        let (path, msg) = build_self_clear_message(
            &cwd,
            "msg-sc-shell",
            "rid-sc-shell",
            Some(token.to_string()),
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .unwrap();

        let reason = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-shell.reason.txt");
        assert!(reason.exists(), "non-coding shell must be rejected");
        assert_eq!(pending_self_clear_len(&app).await, 0);
    }

    #[tokio::test]
    async fn handle_self_clear_root_agent_is_allowed() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let cwd = temp.path().join("agent-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let (session_id, token) =
            seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
        seed_self_handoff(&cwd); // #626 existence gate
                                 // #617 follow-up: the Root Agent exclusion was removed. A token-authorized
                                 // self-clear from the Root must now queue like any other coding-agent
                                 // session. Identity is still resolved solely by find_by_token, so this can
                                 // only ever clear the session that owns the presented token.
        {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.set_is_root_agent(session_id, true).await;
        }

        let (path, msg) =
            build_self_clear_message(&cwd, "msg-sc-root", "rid-sc-root", Some(token.to_string()));
        let poller = MailboxPoller::new();
        poller
            .handle_self_clear(&app, &path, &msg, false)
            .await
            .unwrap();

        // Queued, not rejected: the response is "queued", exactly one id is pending,
        // and no reject reason file was written for the Root session.
        assert_eq!(
            read_self_clear_response_status(&cwd, "rid-sc-root").as_deref(),
            Some("queued")
        );
        assert!(pending_self_clear_contains(&app, session_id).await);
        assert_eq!(pending_self_clear_len(&app).await, 1);
        let reason = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-root.reason.txt");
        assert!(
            !reason.exists(),
            "root self-clear must NOT be rejected anymore"
        );
    }

    /// Build a self-clear outbox message with an explicit `from`, written under
    /// `<cwd>/<local-dir>/outbox/`. The canonical OutboxMessage literal for the
    /// self-clear tests lives here; `build_self_clear_message` delegates with the
    /// fixed dev-rust sender, and the Root e2e tests pass either the corrected
    /// sender (ROOT_AGENT_SENDER) or the buggy path-derived value. Returns the
    /// on-disk path and the message; the `process_message` driver reads the message
    /// back from disk and ignores the returned struct.
    fn build_self_clear_message_with_from(
        cwd: &Path,
        msg_id: &str,
        request_id: &str,
        token: Option<String>,
        from: &str,
    ) -> (PathBuf, OutboxMessage) {
        let outbox_dir = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        std::fs::create_dir_all(&outbox_dir).unwrap();
        let path = outbox_dir.join(format!("{}.json", msg_id));
        let msg = OutboxMessage {
            id: msg_id.into(),
            token,
            from: from.into(),
            to: String::new(),
            body: String::new(),
            mode: String::new(),
            get_output: false,
            request_id: Some(request_id.into()),
            sender_agent: None,
            preferred_agent: String::new(),
            priority: "normal".into(),
            timestamp: "2026-06-24T00:00:00Z".into(),
            command: None,
            action: Some(SELF_CLEAR_ACTION.into()),
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: None,
            switch_profile: None,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
        (path, msg)
    }

    struct SelfSwitchFixture {
        _temp: tempfile::TempDir,
        replica: PathBuf,
        _origin: PathBuf,
        app: tauri::App<tauri::test::MockRuntime>,
    }

    fn make_self_switch_fixture() -> SelfSwitchFixture {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let workspace = project.join(".ac");
        let origin = workspace.join("_agent_dev-rust");
        let wg_dir = workspace.join("wg-1-dev-team");
        let replica = wg_dir.join("__agent_dev-rust");

        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let app = make_mailbox_app(temp.path());
        SelfSwitchFixture {
            _temp: temp,
            replica,
            _origin: origin,
            app,
        }
    }

    async fn seed_self_switch_session(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        cwd: &Path,
        shell: &str,
        agent_id: Option<&str>,
        requested_profile: Option<&str>,
        effective_profile: Option<&str>,
    ) -> (Uuid, Uuid) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr
            .create_session(
                shell.into(),
                vec![],
                cwd.to_string_lossy().to_string(),
                agent_id.map(str::to_string),
                agent_id.map(|id| format!("Label for {}", id)),
                Vec::new(),
                false,
            )
            .await
            .unwrap();
        mgr.set_profile_metadata(
            session.id,
            requested_profile.map(str::to_string),
            effective_profile.map(str::to_string),
            Vec::new(),
            false,
            None,
            None,
        )
        .await;
        (session.id, session.token)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_self_switch_message(
        cwd: &Path,
        msg_id: &str,
        request_id: &str,
        token: Option<String>,
        coding_agent: Option<&str>,
        profile: Option<&str>,
        from: &str,
    ) -> (PathBuf, OutboxMessage) {
        let outbox_dir = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        std::fs::create_dir_all(&outbox_dir).unwrap();
        let path = outbox_dir.join(format!("{}.json", msg_id));
        let msg = OutboxMessage {
            id: msg_id.into(),
            token,
            from: from.into(),
            to: String::new(),
            body: String::new(),
            mode: String::new(),
            get_output: false,
            request_id: Some(request_id.into()),
            sender_agent: None,
            preferred_agent: String::new(),
            priority: "normal".into(),
            timestamp: "2026-06-28T00:00:00Z".into(),
            command: None,
            action: Some(SELF_SWITCH_ACTION.into()),
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: coding_agent.map(str::to_string),
            switch_profile: profile.map(str::to_string),
        };
        std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
        (path, msg)
    }

    fn read_reject_reason(cwd: &Path, msg_id: &str) -> Option<String> {
        std::fs::read_to_string(
            cwd.join(crate::config::agent_local_dir_name())
                .join("outbox")
                .join("rejected")
                .join(format!("{}.reason.txt", msg_id)),
        )
        .ok()
    }

    #[tokio::test]
    async fn handle_self_switch_valid_token_queues_with_resolved_targets() {
        let fixture = make_self_switch_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) = seed_self_switch_session(
            &app,
            &fixture.replica,
            "codex",
            Some("codex"),
            Some("A"),
            Some("B"),
        )
        .await;
        seed_self_handoff(&fixture.replica);
        let forget_content = "topic to forget\nwith full switch archive content";
        std::fs::write(fixture.replica.join("SELF-FORGET.md"), forget_content).unwrap();

        let (path, msg) = build_self_switch_message(
            &fixture.replica,
            "msg-ss-1",
            "rid-ss-1",
            Some(token.to_string()),
            Some("claude"),
            Some("c"),
            "proj-a:wg-1-dev-team/dev-rust",
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_handoff_switch(&app, &path, &msg, false)
            .await
            .unwrap();

        let response = read_response_json(&fixture.replica, "rid-ss-1").unwrap();
        assert_eq!(response["action"], SELF_SWITCH_ACTION);
        assert_eq!(response["status"], "queued");
        assert_eq!(response["target_coding_agent"], "claude");
        assert_eq!(response["target_profile"], "C");
        assert!(pending_self_clear_contains(&app, session_id).await);
        assert_eq!(pending_self_clear_len(&app).await, 1);
        assert_eq!(count_forget_archives(&fixture.replica), 1);
        assert_eq!(read_only_forget_archive(&fixture.replica), forget_content);
        assert!(!fixture.replica.join("SELF-FORGET.md").is_file());
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(
            saved["tooling"]["currentCodingAgent"].is_null(),
            "target selection is persisted by Phase 1, not queue time"
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn handle_self_switch_pending_alias_reports_already_queued() {
        let fixture = make_self_switch_fixture();
        let app = app_handle(&fixture.app);
        let (session_id, token) = seed_self_switch_session(
            &app,
            &fixture.replica,
            "codex",
            Some("codex"),
            None,
            Some("A"),
        )
        .await;
        seed_self_handoff(&fixture.replica);
        std::fs::write(fixture.replica.join("SELF-FORGET.md"), "new topic").unwrap();
        {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            pending
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(session_id);
        }

        let (path, msg) = build_self_switch_message(
            &fixture.replica,
            "msg-ss-alias",
            "rid-ss-alias",
            Some(token.to_string()),
            None,
            None,
            "proj-a:wg-1-dev-team/dev-rust",
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_handoff_switch(&app, &path, &msg, false)
            .await
            .unwrap();

        let response = read_response_json(&fixture.replica, "rid-ss-alias").unwrap();
        assert_eq!(response["status"], "already_queued");
        assert_eq!(response["target_coding_agent"], "codex");
        assert_eq!(response["target_profile"], "A");
        assert_eq!(pending_self_clear_len(&app).await, 1);
        assert!(
            fixture.replica.join("SELF-FORGET.md").is_file(),
            "already_queued must not archive a new SELF-FORGET.md"
        );
    }

    #[tokio::test]
    async fn handle_self_switch_rejects_origin_before_handoff_gate() {
        let fixture = make_self_switch_fixture();
        let app = app_handle(&fixture.app);
        let (_session_id, token) = seed_self_switch_session(
            &app,
            &fixture._origin,
            "codex",
            Some("codex"),
            None,
            Some("A"),
        )
        .await;

        let (path, msg) = build_self_switch_message(
            &fixture._origin,
            "msg-ss-origin",
            "rid-ss-origin",
            Some(token.to_string()),
            None,
            None,
            "proj-a/dev-rust",
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_handoff_switch(&app, &path, &msg, false)
            .await
            .unwrap();

        let reason = read_reject_reason(&fixture._origin, "msg-ss-origin").unwrap();
        assert!(reason.contains("WG replica"), "{reason}");
        assert!(!reason.contains("SELF-HANDOFF"), "{reason}");
        assert_eq!(pending_self_clear_len(&app).await, 0);
    }

    #[tokio::test]
    async fn handle_self_switch_rejects_fake_replica_before_handoff_gate() {
        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let fake = temp.path().join("__agent_fake");
        std::fs::create_dir_all(&fake).unwrap();
        let (_session_id, token) =
            seed_self_switch_session(&app, &fake, "codex", Some("codex"), None, Some("A")).await;

        let (path, msg) = build_self_switch_message(
            &fake,
            "msg-ss-fake",
            "rid-ss-fake",
            Some(token.to_string()),
            None,
            None,
            "proj-a:wg-1-dev-team/fake",
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_handoff_switch(&app, &path, &msg, false)
            .await
            .unwrap();

        let reason = read_reject_reason(&fake, "msg-ss-fake").unwrap();
        assert!(reason.contains("configured WG replica"), "{reason}");
        assert!(!reason.contains("SELF-HANDOFF"), "{reason}");
        assert_eq!(pending_self_clear_len(&app).await, 0);
    }

    #[tokio::test]
    async fn handle_self_switch_unknown_coding_agent_lists_configured_ids() {
        let fixture = make_self_switch_fixture();
        let app = app_handle(&fixture.app);
        {
            let settings = app.state::<SettingsState>();
            let mut cfg = settings.write().await;
            cfg.agents = vec![
                wake_agent("codex-main", "Codex Main", "codex"),
                wake_agent(
                    "codex-research",
                    "Codex Research",
                    "codex --profile research",
                ),
            ];
        }
        let (_session_id, token) = seed_self_switch_session(
            &app,
            &fixture.replica,
            "codex",
            Some("codex-main"),
            None,
            Some("A"),
        )
        .await;
        seed_self_handoff(&fixture.replica);

        let (path, msg) = build_self_switch_message(
            &fixture.replica,
            "msg-ss-unknown",
            "rid-ss-unknown",
            Some(token.to_string()),
            Some("codex"),
            None,
            "proj-a:wg-1-dev-team/dev-rust",
        );
        let poller = MailboxPoller::new();
        poller
            .handle_self_handoff_switch(&app, &path, &msg, false)
            .await
            .unwrap();

        let reason = read_reject_reason(&fixture.replica, "msg-ss-unknown").unwrap();
        assert!(reason.contains("codex-main (Codex Main)"), "{reason}");
        assert!(
            reason.contains("codex-research (Codex Research)"),
            "{reason}"
        );
        assert_eq!(pending_self_clear_len(&app).await, 0);
    }

    #[tokio::test]
    async fn self_clear_driver_injects_first_captured_summary_not_later_file() {
        let session_id = Uuid::new_v4();
        let pending = Arc::new(crate::PendingSelfClear::default());
        pending.0.lock().unwrap().insert(session_id);

        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("SELF-FORGET.md"), "first queued summary").unwrap();
        let forgotten_summary = capture_self_forget_summary(temp.path());
        archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive first forget");
        std::fs::write(temp.path().join("SELF-FORGET.md"), "second later summary").unwrap();

        let injected = Arc::new(Mutex::new(Vec::<String>::new()));
        let archived = Arc::new(Mutex::new(Vec::<Uuid>::new()));

        let session_state = move |_session_id: Uuid| async move { (true, true) };
        let injected_seen = injected.clone();
        let inject = move |_session_id: Uuid, prompt: String| {
            let injected_seen = injected_seen.clone();
            async move {
                injected_seen.lock().unwrap().push(prompt);
                Ok(())
            }
        };
        let archived_seen = archived.clone();
        let archive = move |session_id: Uuid, _delay: std::time::Duration| {
            archived_seen.lock().unwrap().push(session_id);
        };

        MailboxPoller::drive_self_clear_after_sustained_idle(
            session_id,
            pending.clone(),
            forgotten_summary,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            session_state,
            inject,
            archive,
        )
        .await;

        let injected = injected.lock().unwrap().clone();
        assert_eq!(injected.len(), 2);
        assert_eq!(injected[0], "/clear");
        assert!(injected[1].contains("first queued summary"));
        assert!(!injected[1].contains("second later summary"));
        assert!(injected[1].contains("closed background"));
        assert!(injected[1].contains("active core information"));
        assert!(!injected[1].contains('\n'));
        assert!(!injected[1].contains('\u{2014}'));
        assert_eq!(*archived.lock().unwrap(), vec![session_id]);
        assert!(pending.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn self_switch_driver_restarts_uses_new_id_and_cleans_pending() {
        let original_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();
        let pending = Arc::new(crate::PendingSelfClear::default());
        pending.0.lock().unwrap().insert(original_id);
        let seen_states = Arc::new(Mutex::new(Vec::<Uuid>::new()));
        let persist_calls = Arc::new(Mutex::new(Vec::<(PathBuf, String, String)>::new()));
        let restart_calls = Arc::new(Mutex::new(Vec::<Uuid>::new()));
        let inject_calls = Arc::new(Mutex::new(Vec::<Uuid>::new()));
        let archive_calls = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let alias_seen_at_inject = Arc::new(Mutex::new(false));
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("SELF-FORGET.md"), "first switch summary").unwrap();
        let forgotten_summary = capture_self_forget_summary(temp.path());
        archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
            .unwrap()
            .expect("archive first forget");
        std::fs::write(temp.path().join("SELF-FORGET.md"), "second switch summary").unwrap();

        let state_seen = seen_states.clone();
        let session_state = move |session_id: Uuid| {
            let state_seen = state_seen.clone();
            async move {
                state_seen.lock().unwrap().push(session_id);
                (true, true)
            }
        };

        let persist_seen = persist_calls.clone();
        let persist = move |cwd: PathBuf, agent: String, profile: String| {
            let persist_seen = persist_seen.clone();
            async move {
                persist_seen.lock().unwrap().push((cwd, agent, profile));
                Ok(())
            }
        };

        let restart_seen = restart_calls.clone();
        let restart = move |session_id: Uuid, _agent: String, _profile: String| {
            let restart_seen = restart_seen.clone();
            async move {
                restart_seen.lock().unwrap().push(session_id);
                Ok(new_id.to_string())
            }
        };

        let pending_for_inject = pending.clone();
        let inject_seen = inject_calls.clone();
        let alias_seen = alias_seen_at_inject.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            let pending_for_inject = pending_for_inject.clone();
            let inject_seen = inject_seen.clone();
            let alias_seen = alias_seen.clone();
            async move {
                assert!(prompt.contains("first switch summary"));
                assert!(!prompt.contains("second switch summary"));
                assert!(prompt.contains("closed background"));
                assert!(prompt.contains("active core information"));
                assert!(!prompt.contains('\n'));
                assert!(!prompt.contains('\u{2014}'));
                inject_seen.lock().unwrap().push(session_id);
                let set = pending_for_inject
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *alias_seen.lock().unwrap() =
                    set.contains(&original_id) && set.contains(&session_id);
                Ok(())
            }
        };

        let archive_seen = archive_calls.clone();
        let archive = move |root: PathBuf, _delay: std::time::Duration| {
            archive_seen.lock().unwrap().push(root);
        };
        let cwd = temp.path().to_path_buf();

        MailboxPoller::drive_self_switch_after_sustained_idle(
            original_id,
            cwd.clone(),
            "claude".into(),
            "B".into(),
            forgotten_summary,
            pending.clone(),
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            session_state,
            persist,
            restart,
            inject,
            archive,
        )
        .await;

        assert_eq!(*seen_states.lock().unwrap(), vec![original_id, new_id]);
        assert_eq!(
            *persist_calls.lock().unwrap(),
            vec![(cwd.clone(), "claude".into(), "B".into())]
        );
        assert_eq!(*restart_calls.lock().unwrap(), vec![original_id]);
        assert_eq!(*inject_calls.lock().unwrap(), vec![new_id]);
        assert_eq!(*archive_calls.lock().unwrap(), vec![cwd]);
        assert!(
            *alias_seen_at_inject.lock().unwrap(),
            "the new session id must be marked pending during Phase 2 injection"
        );
        assert!(pending.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn self_switch_driver_restart_failure_skips_inject_and_cleans_pending() {
        let original_id = Uuid::new_v4();
        let pending = Arc::new(crate::PendingSelfClear::default());
        pending.0.lock().unwrap().insert(original_id);
        let persist_calls = Arc::new(Mutex::new(0usize));
        let restart_calls = Arc::new(Mutex::new(0usize));
        let inject_calls = Arc::new(Mutex::new(0usize));

        let session_state = move |_session_id: Uuid| async move { (true, true) };
        let persist_seen = persist_calls.clone();
        let persist = move |_cwd: PathBuf, _agent: String, _profile: String| {
            let persist_seen = persist_seen.clone();
            async move {
                *persist_seen.lock().unwrap() += 1;
                Ok(())
            }
        };
        let restart_seen = restart_calls.clone();
        let restart = move |_session_id: Uuid, _agent: String, _profile: String| {
            let restart_seen = restart_seen.clone();
            async move {
                *restart_seen.lock().unwrap() += 1;
                Err("destroyed but not recreated".to_string())
            }
        };
        let inject_seen = inject_calls.clone();
        let inject = move |_session_id: Uuid, _prompt: String| {
            let inject_seen = inject_seen.clone();
            async move {
                *inject_seen.lock().unwrap() += 1;
                Ok(())
            }
        };
        let archive = move |_root: PathBuf, _delay: std::time::Duration| {};

        MailboxPoller::drive_self_switch_after_sustained_idle(
            original_id,
            PathBuf::from("C:/x/.ac/wg-1-dev-team/__agent_dev-rust"),
            "claude".into(),
            "B".into(),
            ForgottenSummary::from_raw("should not inject"),
            pending.clone(),
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            session_state,
            persist,
            restart,
            inject,
            archive,
        )
        .await;

        assert_eq!(*persist_calls.lock().unwrap(), 1);
        assert_eq!(*restart_calls.lock().unwrap(), 1);
        assert_eq!(*inject_calls.lock().unwrap(), 0);
        assert!(pending.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn self_switch_driver_persist_failure_skips_restart_and_cleans_pending() {
        let original_id = Uuid::new_v4();
        let pending = Arc::new(crate::PendingSelfClear::default());
        pending.0.lock().unwrap().insert(original_id);
        let restart_calls = Arc::new(Mutex::new(0usize));
        let inject_calls = Arc::new(Mutex::new(0usize));

        let session_state = move |_session_id: Uuid| async move { (true, true) };
        let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move {
            Err("config write failed".to_string())
        };
        let restart_seen = restart_calls.clone();
        let restart = move |_session_id: Uuid, _agent: String, _profile: String| {
            let restart_seen = restart_seen.clone();
            async move {
                *restart_seen.lock().unwrap() += 1;
                Ok(Uuid::new_v4().to_string())
            }
        };
        let inject_seen = inject_calls.clone();
        let inject = move |_session_id: Uuid, _prompt: String| {
            let inject_seen = inject_seen.clone();
            async move {
                *inject_seen.lock().unwrap() += 1;
                Ok(())
            }
        };
        let archive = move |_root: PathBuf, _delay: std::time::Duration| {};

        MailboxPoller::drive_self_switch_after_sustained_idle(
            original_id,
            PathBuf::from("C:/x/.ac/wg-1-dev-team/__agent_dev-rust"),
            "claude".into(),
            "B".into(),
            ForgottenSummary::from_raw("should not inject"),
            pending.clone(),
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            session_state,
            persist,
            restart,
            inject,
            archive,
        )
        .await;

        assert_eq!(*restart_calls.lock().unwrap(), 0);
        assert_eq!(*inject_calls.lock().unwrap(), 0);
        assert!(pending.0.lock().unwrap().is_empty());
    }

    /// Best-effort removal of one msg-id's outbox artifacts so the Root e2e tests are
    /// idempotent across runs. They must write under the process-global
    /// `root_agent_dir()` (a fixed path, NOT a throwaway TempDir - see the test docs
    /// for why), so stale delivered/rejected/response files from a prior run could
    /// otherwise skew assertions. Each test uses a unique msg-id, so this is safe
    /// even when the two tests run in parallel.
    ///
    /// #626 NOTE: this helper deliberately does NOT touch `SELF-HANDOFF.md`. That file is a single
    /// SHARED (non-msg-id-scoped) name; if this start-of-test cleanup removed it, the negative e2e
    /// (which rejects at anti-spoof and never seeds it) could delete the positive e2e's freshly-seeded
    /// handoff file mid-flight when the two run in parallel - a flaky failure. The positive e2e owns
    /// `SELF-HANDOFF.md` end-to-end instead (seed before, remove after), so no other test races it.
    /// `self-clear/*_SELF-FORGET.md` is glob-removed defensively (the e2e tests never seed SELF-FORGET.md,
    /// so the archive is a no-op and nothing is normally created - this only guards against a stale file
    /// from a crash).
    fn clear_root_self_clear_artifacts(cwd: &Path, msg_id: &str, request_id: &str) {
        let outbox = cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        let _ = std::fs::remove_file(outbox.join(format!("{}.json", msg_id)));
        let _ = std::fs::remove_file(outbox.join("delivered").join(format!("{}.json", msg_id)));
        let _ = std::fs::remove_file(
            outbox
                .join("rejected")
                .join(format!("{}.reason.txt", msg_id)),
        );
        let _ = std::fs::remove_file(outbox.join("rejected").join(format!("{}.json", msg_id)));
        let _ = std::fs::remove_file(
            cwd.join(crate::config::agent_local_dir_name())
                .join("responses")
                .join(format!("{}.json", request_id)),
        );
        // Defensive: drop any stale <ts>_SELF-FORGET.md archive under the shared root's self-clear/ (none
        // is created in the normal e2e flow since neither test seeds SELF-FORGET.md).
        if let Ok(rd) = std::fs::read_dir(cwd.join("self-clear")) {
            for entry in rd.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with("_SELF-FORGET.md")
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// #617 HIGH-1 e2e (production-faithful): a token-authorized Root self-clear
    /// travels the FULL `process_message` agent-outbox path (NOT `handle_self_clear`
    /// directly) and ENQUEUES.
    ///
    /// Fidelity, verified against the code (not assumed):
    /// - A real Root Agent runs `self-clear --token <session-UUID>`. That UUID is
    ///   neither the root_token nor the master token, so `validate_cli_token`
    ///   (cli/mod.rs) returns is_root=false and `self_clear::execute` writes to the
    ///   Root's AGENT outbox; the poller processes agent outboxes with
    ///   `is_app_outbox=false`. So this test passes `false` and BOTH anti-spoof gates
    ///   run, exactly like production. (An earlier draft used `true`, which skips the
    ///   outbox-sender gate and is NOT the real path.)
    /// - With the Root cwd both gates derive ROOT_AGENT_SENDER (outbox-sender via
    ///   `sender_name_for_session_cwd`, token-root via the root-flagged variant), so
    ///   the corrected sender passes both. The cwd MUST be the process-global
    ///   `root_agent_dir()` so `is_root_agent_path` returns true; `config_dir()` /
    ///   `root_agent_dir()` are OnceLock-cached, so a throwaway TempDir cannot stand
    ///   in. Under `cargo test`, `root_agent_dir()` resolves beneath the test
    ///   binary's target dir (never a real install) and the flow only ADDS files, so
    ///   it is isolated and non-destructive.
    /// - `from` is computed by `resolve_self_clear_sender` - the SAME helper
    ///   `execute` uses - so this is a genuine gate: revert the fix and the helper
    ///   yields the path FQN, the daemon rejects it, and the enqueue assertion fails.
    #[tokio::test]
    async fn process_message_root_self_clear_with_canonical_sender_queues() {
        let root_cwd = PathBuf::from(
            crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"),
        );
        std::fs::create_dir_all(&root_cwd).unwrap();
        clear_root_self_clear_artifacts(&root_cwd, "msg-sc-root-e2e-pos", "rid-sc-root-e2e-pos");
        // #626 existence gate: seed SELF-HANDOFF.md so the positive path still queues. This test OWNS
        // SELF-HANDOFF.md in the shared root_agent_dir (the negative e2e never touches it), so seeding
        // here and removing at the end is race-free even with parallel test execution. NOTE: deliberately
        // do NOT seed SELF-FORGET.md here (keep the archive a no-op so no timestamped litter in the shared dir).
        seed_self_handoff(&root_cwd);

        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let (session_id, token) =
            seed_self_clear_session(&app, &root_cwd.to_string_lossy(), "claude").await;
        {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.set_is_root_agent(session_id, true).await;
        }

        // The exact `from` the corrected CLI stamps for the Root cwd.
        let canonical_from =
            crate::cli::self_clear::resolve_self_clear_sender(&root_cwd.to_string_lossy());
        assert_eq!(
            canonical_from,
            crate::config::root_agent::ROOT_AGENT_SENDER,
            "fix precondition: the CLI must stamp ROOT_AGENT_SENDER for the Root cwd"
        );

        let (path, _msg) = build_self_clear_message_with_from(
            &root_cwd,
            "msg-sc-root-e2e-pos",
            "rid-sc-root-e2e-pos",
            Some(token.to_string()),
            &canonical_from,
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, false)
            .await
            .expect("process_message should succeed");

        // Enqueued: the queue-ack response is "queued", exactly one id is pending, and
        // no reject reason file was written by either anti-spoof gate.
        assert_eq!(
            read_self_clear_response_status(&root_cwd, "rid-sc-root-e2e-pos").as_deref(),
            Some("queued")
        );
        assert!(
            pending_self_clear_contains(&app, session_id).await,
            "canonical Root sender must enqueue the self-clear"
        );
        assert_eq!(pending_self_clear_len(&app).await, 1);
        let reason = root_cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-root-e2e-pos.reason.txt");
        assert!(
            !reason.exists(),
            "canonical Root sender must NOT be rejected by either anti-spoof gate"
        );
        assert!(
            !path.exists(),
            "message must be consumed (moved to delivered/)"
        );

        // #626: this test owns SELF-HANDOFF.md in the shared root; remove it so it does not linger.
        let _ = std::fs::remove_file(root_cwd.join("SELF-HANDOFF.md"));
    }

    /// #617 HIGH-1 e2e negative (production-faithful): the EXACT buggy sender the old
    /// CLI produced - `agent_name_from_root(<Root cwd>)`, a path-derived FQN, not
    /// ROOT_AGENT_SENDER - is REJECTED on the same full `process_message` agent-outbox
    /// path, so nothing queues. This is why the capability was dead in production
    /// before the fix. In the agent-outbox path the outbox-sender gate is the first to
    /// fire (the buggy `from` does not match the Root-derived outbox owner); the
    /// token-root gate would reject it too. Proves the `from` value is load-bearing.
    #[tokio::test]
    async fn process_message_root_self_clear_with_buggy_sender_is_rejected() {
        let root_cwd = PathBuf::from(
            crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"),
        );
        std::fs::create_dir_all(&root_cwd).unwrap();
        clear_root_self_clear_artifacts(&root_cwd, "msg-sc-root-e2e-neg", "rid-sc-root-e2e-neg");

        let temp = tempfile::TempDir::new().unwrap();
        let app_struct = make_mailbox_app(temp.path());
        let app = app_handle(&app_struct);
        let (session_id, token) =
            seed_self_clear_session(&app, &root_cwd.to_string_lossy(), "claude").await;
        {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.set_is_root_agent(session_id, true).await;
        }

        // Reproduce the EXACT value the pre-fix CLI stamped for the Root cwd.
        let buggy_from = crate::cli::send::agent_name_from_root(&root_cwd.to_string_lossy());
        assert_ne!(
            buggy_from,
            crate::config::root_agent::ROOT_AGENT_SENDER,
            "precondition: the pre-fix sender must differ from the canonical Root sender"
        );

        let (path, _msg) = build_self_clear_message_with_from(
            &root_cwd,
            "msg-sc-root-e2e-neg",
            "rid-sc-root-e2e-neg",
            Some(token.to_string()),
            &buggy_from,
        );
        let poller = MailboxPoller::new();
        poller
            .process_message(&app, &path, false)
            .await
            .expect("process_message returns Ok even when it rejects");

        // Rejected by an anti-spoof gate (mismatch); nothing queued.
        let reason_path = root_cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
            .join("rejected")
            .join("msg-sc-root-e2e-neg.reason.txt");
        let reason = std::fs::read_to_string(&reason_path)
            .expect("buggy Root sender must be rejected with a reason file");
        assert!(
            reason.contains("mismatch"),
            "expected an anti-spoof mismatch rejection, got: {reason}"
        );
        assert_eq!(pending_self_clear_len(&app).await, 0);
        assert!(
            !pending_self_clear_contains(&app, session_id).await,
            "buggy Root sender must not enqueue anything"
        );
    }

    // ── err_is_pty_session_missing tests (issue #223) ──

    #[test]
    fn err_is_pty_session_missing_matches_inject_wrap() {
        // Exact shape emitted by pty::inject::inject_text_into_session
        // (pty/inject.rs:67) when PtyManager::write returns SessionNotFound.
        let e = "PTY write failed: Session not found: 2ced5ccf-1234-5678-9abc-def012345678";
        assert!(err_is_pty_session_missing(e));
    }

    #[test]
    fn err_is_pty_session_missing_matches_bare_form() {
        // Matches even without the inject_text_into_session wrapping, in case
        // a future call site propagates the inner error directly.
        let e = "Session not found: abcdef";
        assert!(err_is_pty_session_missing(e));
    }

    #[test]
    fn err_is_pty_session_missing_rejects_unrelated_errors() {
        assert!(!err_is_pty_session_missing("PTY error: broken pipe"));
        assert!(!err_is_pty_session_missing(
            "Failed to read outbox file: foo"
        ));
        assert!(!err_is_pty_session_missing(""));
    }

    /// Pins the actual `AppError::SessionNotFound` Display format. If a future
    /// refactor changes `errors.rs:5` to anything other than `"Session not
    /// found: {0}"`, this test fails with a clear message — the sniff is the
    /// load-bearing safety net for the candidate loop. (grinch G.M3.)
    #[test]
    fn err_is_pty_session_missing_matches_actual_apperror_format() {
        use crate::errors::AppError;
        let id = uuid::Uuid::new_v4();
        let raw = AppError::SessionNotFound(id.to_string()).to_string();
        assert!(
            err_is_pty_session_missing(&raw),
            "AppError::SessionNotFound display changed to {:?}; \
             err_is_pty_session_missing substring sniff broken — \
             update both the sniff and this test",
            raw
        );
    }

    // ── Anti-spoof / canonicalization pure-logic tests (AR2-tests 22, 23 + DR2-5) ──

    /// §DR7 / AR2-tests #22: legacy-unqualified msg.from is accepted when its
    /// local part matches the anti-spoof expected_from (local-only fallback).
    #[test]
    fn anti_spoof_legacy_msg_from_accepted_by_local_match() {
        // expected_from derived from repo path (canonical FQN).
        let expected = "proj-a:wg-1-devs/tech-lead";
        // msg.from is legacy unqualified with same local part.
        let msg_from = "wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(msg_from, expected));
    }

    /// §DR7 / AR2-tests #23: qualified-but-wrong-project msg.from is rejected.
    /// A naïve suffix match would accept this — the LOCAL-only fallback rejects.
    #[test]
    fn anti_spoof_cross_project_qualified_msg_from_rejected() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        let spoofed = "proj-b:wg-1-devs/tech-lead";
        assert!(!anti_spoof_accept(spoofed, expected));
    }

    /// Exact-FQN match is trivially accepted (baseline).
    #[test]
    fn anti_spoof_exact_fqn_match_accepted() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(expected, expected));
    }

    /// §DR2-5 / AR2-tests #25: §AR2-norm step (1) upgrades a legacy-unqualified
    /// msg.from to the anti-spoof expected_from FQN. Without this upgrade,
    /// grinch §G5's `resolve_repo_path(&msg.from)` response-dir lookup could
    /// receive an ambiguous-local-part input.
    #[test]
    fn process_message_canonicalizes_legacy_msg_from() {
        let mut msg_from = "wg-1-devs/tech-lead".to_string();
        let expected_from = "proj-a:wg-1-devs/tech-lead";
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, Some(expected_from));
        assert!(upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// Already-qualified msg.from is NOT overwritten by canonicalization.
    #[test]
    fn canonicalize_noop_for_already_qualified_msg_from() {
        let mut msg_from = "proj-a:wg-1-devs/tech-lead".to_string();
        let upgraded = canonicalize_msg_from_in_place(
            &mut msg_from,
            Some("proj-b:wg-1-devs/tech-lead"), // different project — don't overwrite!
        );
        assert!(!upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// No expected_from (app outbox path) → canonicalization is a no-op.
    #[test]
    fn canonicalize_noop_when_expected_from_absent() {
        let mut msg_from = "wg-1-devs/alice".to_string();
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, None);
        assert!(!upgraded);
        assert_eq!(msg_from, "wg-1-devs/alice");
    }

    // ── Full mailbox-pipeline tests marked [INT] — placeholders for a future
    // two-project fixture harness. Acknowledged to ship with the fix per
    // tech-lead's must-apply directive; the pure-logic tests above cover the
    // §G1, §G2, §G5 regression surface at the unit level, and §AR2-G1's
    // close-session resolver gate is covered by the §AR2-shared resolver
    // tests (config::teams::tests::resolve_agent_target_*). ──

    /// §G9#1 / AR2-tests #17 — close-session with unqualified target from a
    /// direct outbox write MUST NOT destroy sessions in an unauthorized
    /// project. Covered at the resolver layer by
    /// `resolve_agent_target_rejects_ambiguous` and
    /// `resolve_agent_target_two_level_scan` in `config/teams.rs::tests`;
    /// `handle_close_session`'s §AR2-G1 gate calls `resolve_agent_target`
    /// before authorization, so rejecting Ambiguous at that layer blocks the
    /// attack before any session is touched. Full end-to-end fixture needs a
    /// Tauri `AppHandle` harness — follow-up.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn close_session_rejects_direct_outbox_write_with_unqualified_target() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
        //   - config::teams::tests::is_coordinator_rejects_legacy_unqualified_from
    }

    /// §G9#2 / AR2-tests #18 — wake with ambiguous unqualified `to` MUST be
    /// rejected, not silently routed. Covered at the resolver layer by the
    /// same `resolve_agent_target_rejects_ambiguous` test; `process_message`
    /// calls `resolve_agent_target` on `msg.to` at §AR2-norm before mode
    /// dispatch, so ambiguous `msg.to` becomes a rejected outbox message.
    /// Full end-to-end fixture needs an AppHandle harness.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn deliver_wake_rejects_unqualified_to_with_cross_project_matches() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
    }

    /// §G9#3 / AR2-tests #19 — `resolve_repo_path` WG fallback with a
    /// qualified target honors `target_project` (no cross-project leak even
    /// when the base dir `rp` is another project's root). Logic covered by
    /// the `project_matches` closure + `dirs_to_check` seeding in the
    /// §AR2-G3 block; a full fixture-based test needs filesystem setup
    /// under an AppHandle harness.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_wg_fallback_honors_target_project() {}

    /// §G9#4 / AR2-tests #20 — `resolve_repo_path` with an unqualified target
    /// matching multiple projects returns `None` (refuses arbitrary pick).
    /// §AR2-G4 collector pattern logic is covered by inspection — the
    /// `matches.len()` match arm returns None on `_`. Full integration test
    /// needs AppHandle + session-CWDs fixture.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_returns_none_on_ambiguous_unqualified() {}

    /// §G9#8 / AR2-tests #21 — a session spawned by `deliver_wake` from an
    /// FQN `msg.to` has a sidebar `Session.name` WITHOUT the `:` prefix.
    /// §AR2-session-name handles this at mailbox.rs (spawn path) via
    /// `split_project_prefix(&msg.to).1`. The logic is one line; a full
    /// integration test needs a Tauri runtime.
    #[test]
    #[ignore = "integration: needs Tauri runtime + session manager fixture"]
    fn deliver_wake_spawned_session_name_has_no_colon() {}

    /// §G9#9 / AR2-tests #24 — full round-trip CLI send → mailbox route →
    /// reply. Intentionally integration-level; no unit scaffolding.
    #[test]
    #[ignore = "integration: full CLI + mailbox + two-project fixture"]
    fn resolve_to_target_round_trip_integration() {}

    // ── #228 D1-a — derive_project_from_outbox_path tests ──

    #[test]
    fn derive_project_from_outbox_path_walks_up_wg_replica_layout() {
        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("proj-x");
        let outbox = project_dir
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice")
            .join(".agentscommander") // matches agent_local_dir_name() shape
            .join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let file = outbox.join("msg-42.json");
        std::fs::write(&file, "{}").unwrap();

        let got = derive_project_from_outbox_path(&file)
            .unwrap()
            .expect("should derive project from WG layout");
        let expected = std::fs::canonicalize(&project_dir)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_project_from_outbox_path_accepts_ac_outbox() {
        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("proj-x");
        let outbox = project_dir
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice")
            .join(".agentscommander")
            .join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let file = outbox.join("msg-42.json");
        std::fs::write(&file, "{}").unwrap();

        let got = derive_project_from_outbox_path(&file)
            .unwrap()
            .expect(".ac outbox should derive project");
        let expected = std::fs::canonicalize(&project_dir)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(got, expected);
    }

    #[test]
    fn filter_sessions_by_fqn_accepts_ac_live_target() {
        let temp = tempfile::TempDir::new().unwrap();
        let agent = temp
            .path()
            .join("proj-x")
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice");
        std::fs::create_dir_all(&agent).unwrap();

        let pool = vec![make_session_info(
            "uuid-ac",
            "alice-ac",
            &path_string(&agent),
            crate::session::session::SessionStatus::Idle,
            true,
        )];

        let hits = filter_sessions_by_fqn(&pool, "proj-x:wg-1-devs/alice");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "uuid-ac");
    }

    #[test]
    fn wg_session_fallback_returns_none_without_target_agent() {
        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("proj-x");
        let sibling = project_dir
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_bob");
        std::fs::create_dir_all(&sibling).unwrap();

        let dirs = vec![(Uuid::nil(), path_string(&sibling))];

        assert!(resolve_wg_path_from_session_dirs(&dirs, "proj-x:wg-1-devs/alice").is_none());
    }

    #[test]
    fn wg_session_fallback_returns_ac_target_from_sibling() {
        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("proj-x");
        let target = project_dir
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice");
        let sibling = project_dir
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_bob");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();

        let dirs = vec![(Uuid::nil(), path_string(&sibling))];

        let got = resolve_wg_path_from_session_dirs(&dirs, "proj-x:wg-1-devs/alice")
            .expect(".ac sibling should resolve target");

        assert_eq!(
            std::fs::canonicalize(got).unwrap(),
            std::fs::canonicalize(target).unwrap()
        );
    }

    #[test]
    fn derive_project_from_outbox_path_rejects_non_wg_layout() {
        let temp = tempfile::TempDir::new().unwrap();
        // A path with the right tail but missing the `.ac` ancestor.
        let outbox = temp
            .path()
            .join("random")
            .join(".agentscommander")
            .join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let file = outbox.join("msg.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(derive_project_from_outbox_path(&file).unwrap().is_none());
    }

    #[test]
    fn derive_project_from_outbox_path_rejects_app_outbox_layout() {
        // App-outbox lives at <config_dir>/app-outbox/<file>.json — no
        // `__agent_*` / `wg-*` ancestors. Helper must return None so app-outbox
        // messages are NOT augmented (they have their own master-token bypass).
        let temp = tempfile::TempDir::new().unwrap();
        let app_outbox = temp.path().join("config").join("app-outbox");
        std::fs::create_dir_all(&app_outbox).unwrap();
        let file = app_outbox.join("msg.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(derive_project_from_outbox_path(&file).unwrap().is_none());
    }

    fn write_project_refresh_request_fixture(
        path: &Path,
        id: &str,
        project_path: &Path,
        changed_name: Option<&str>,
        reason: &str,
    ) {
        let request = crate::cli::create_agent_matrix::ProjectRefreshRequest {
            id: id.to_string(),
            project_path: project_path.to_string_lossy().to_string(),
            changed_path: changed_name.map(|_| {
                project_path
                    .join(".ac")
                    .join("_agent_architect")
                    .to_string_lossy()
                    .to_string()
            }),
            changed_name: changed_name.map(str::to_string),
            reason: reason.to_string(),
            timestamp: "2026-05-28T20:00:00Z".to_string(),
        };
        std::fs::write(path, serde_json::to_string_pretty(&request).unwrap()).unwrap();
    }

    #[test]
    fn project_refresh_request_reader_skips_tmp_files() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let project = temp.path().join("ProjectAlpha");
        std::fs::create_dir_all(&project).unwrap();
        let tmp_file = requests_dir.join("request.json.tmp");
        write_project_refresh_request_fixture(
            &tmp_file,
            "request-tmp",
            &project,
            Some("ProjectAlpha/architect"),
            "createAgentMatrix",
        );

        let batch = collect_project_refresh_requests(&requests_dir);

        assert!(batch.payloads.is_empty());
        assert!(batch.processed_paths.is_empty());
        assert!(
            tmp_file.is_file(),
            "tmp file should be left for a future poll"
        );
    }

    #[test]
    fn project_refresh_request_reader_deletes_malformed_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let bad_file = requests_dir.join("bad.json");
        std::fs::write(&bad_file, "{not json").unwrap();

        let batch = collect_project_refresh_requests(&requests_dir);

        assert!(batch.payloads.is_empty());
        assert!(batch.processed_paths.is_empty());
        assert!(!bad_file.exists(), "malformed request should be deleted");
    }

    #[test]
    fn project_refresh_request_reader_coalesces_same_project() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let project = temp.path().join("ProjectAlpha");
        std::fs::create_dir_all(project.join(".ac").join("_agent_architect")).unwrap();
        write_project_refresh_request_fixture(
            &requests_dir.join("a.json"),
            "request-a",
            &project,
            Some("ProjectAlpha/architect"),
            "createAgentMatrix",
        );
        write_project_refresh_request_fixture(
            &requests_dir.join("b.json"),
            "request-b",
            &project,
            Some("ProjectAlpha/planner"),
            "workgroupCreated",
        );

        let batch = collect_project_refresh_requests(&requests_dir);

        assert_eq!(batch.payloads.len(), 1);
        assert_eq!(batch.processed_paths.len(), 2);
        let expected_project = std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(batch.payloads[0].project_path, expected_project);
        assert_eq!(batch.payloads[0].id, "request-a");
    }

    #[test]
    fn project_refresh_request_reader_preserves_registration_payload() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let project = temp.path().join("ProjectAlpha");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        write_project_refresh_request_fixture(
            &requests_dir.join("registration.json"),
            "request-registration",
            &project,
            None,
            "projectRegistered",
        );

        let batch = collect_project_refresh_requests(&requests_dir);

        assert_eq!(batch.payloads.len(), 1);
        let payload = &batch.payloads[0];
        assert_eq!(payload.id, "request-registration");
        assert_eq!(payload.reason, "projectRegistered");
        assert_eq!(payload.changed_path, None);
        assert_eq!(payload.changed_name, None);
    }

    #[test]
    fn project_refresh_request_reader_accepts_legacy_agent_fields() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let project = temp.path().join("ProjectAlpha");
        let agent_path = project.join(".ac").join("_agent_architect");
        std::fs::create_dir_all(&agent_path).unwrap();
        let request = serde_json::json!({
            "id": "legacy-request",
            "projectPath": project.to_string_lossy(),
            "agentPath": agent_path.to_string_lossy(),
            "agentName": "ProjectAlpha/architect",
            "reason": "createAgentMatrix",
            "timestamp": "2026-05-28T20:00:00Z"
        });
        std::fs::write(
            requests_dir.join("legacy.json"),
            serde_json::to_string_pretty(&request).unwrap(),
        )
        .unwrap();

        let batch = collect_project_refresh_requests(&requests_dir);

        assert_eq!(batch.payloads.len(), 1);
        let expected_agent_path = agent_path.to_string_lossy().to_string();
        assert_eq!(
            batch.payloads[0].changed_path.as_deref(),
            Some(expected_agent_path.as_str())
        );
        assert_eq!(
            batch.payloads[0].changed_name.as_deref(),
            Some("ProjectAlpha/architect")
        );
    }

    #[test]
    fn project_refresh_request_reader_prioritizes_registration_for_same_project() {
        let temp = tempfile::TempDir::new().unwrap();
        let requests_dir = temp.path().join("project-refresh-requests");
        std::fs::create_dir_all(&requests_dir).unwrap();
        let project = temp.path().join("ProjectAlpha");
        std::fs::create_dir_all(project.join(".ac").join("_agent_architect")).unwrap();
        write_project_refresh_request_fixture(
            &requests_dir.join("a-mutation.json"),
            "request-mutation",
            &project,
            Some("ProjectAlpha/architect"),
            "createAgentMatrix",
        );
        write_project_refresh_request_fixture(
            &requests_dir.join("z-registration.json"),
            "request-registration",
            &project,
            None,
            "projectRegistered",
        );

        let batch = collect_project_refresh_requests(&requests_dir);

        assert_eq!(batch.payloads.len(), 1);
        assert_eq!(batch.processed_paths.len(), 2);
        assert_eq!(batch.payloads[0].id, "request-registration");
        assert_eq!(batch.payloads[0].reason, "projectRegistered");
        assert_eq!(batch.payloads[0].changed_path, None);
        assert_eq!(batch.payloads[0].changed_name, None);
    }

    // ── Issue #481 mailbox wake-routing integration tests ──

    #[tokio::test]
    async fn deliver_wake_routes_to_live_when_phantom_present() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let phantom_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "phantom",
            SessionStatus::Running,
            None,
        )
        .await;
        let live_id =
            add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(phantom_id, false);
        hooks.pty_presence.lock().unwrap().insert(live_id, true);
        hooks.inject_results.lock().unwrap().push_back(Ok(()));
        let message_path = write_wake_outbox_message_with_route(
            &fixture.sender_cwd,
            "msg-live",
            LOCAL_WAKE_FROM,
            LOCAL_WAKE_TO,
        );

        run_mailbox_message(&app, &message_path, hooks.clone()).await;

        let injected = hooks.inject_calls.lock().unwrap().clone();
        assert_eq!(injected, vec![live_id]);
        assert!(!injected.contains(&phantom_id));
        assert!(hooks.destroy_calls.lock().unwrap().is_empty());
        assert!(hooks.spawn_calls.lock().unwrap().is_empty());
        assert_no_spawn_or_destroy_events(&hooks);
        assert_inject_results_consumed(&hooks);
    }

    #[tokio::test]
    async fn deliver_wake_falls_back_when_first_inject_race_kills_pty() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let first_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "first",
            SessionStatus::Running,
            None,
        )
        .await;
        let second_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "second",
            SessionStatus::Running,
            None,
        )
        .await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(first_id, true);
        hooks.pty_presence.lock().unwrap().insert(second_id, true);
        {
            let mut results = hooks.inject_results.lock().unwrap();
            results.push_back(Err(format!(
                "PTY write failed: Session not found: {}",
                first_id
            )));
            results.push_back(Ok(()));
        }
        let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-race");

        run_mailbox_message(&app, &message_path, hooks.clone()).await;

        assert_eq!(
            *hooks.inject_calls.lock().unwrap(),
            vec![first_id, second_id]
        );
        assert!(hooks.destroy_calls.lock().unwrap().is_empty());
        assert!(hooks.spawn_calls.lock().unwrap().is_empty());
        assert_no_spawn_or_destroy_events(&hooks);
        assert_inject_results_consumed(&hooks);
    }

    #[tokio::test]
    async fn deliver_wake_respawns_exited_deferred_session_with_resume_flag() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let exited_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "deferred",
            SessionStatus::Exited(0),
            None,
        )
        .await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(exited_id, false);
        hooks.inject_results.lock().unwrap().push_back(Ok(()));
        let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-respawn");

        run_mailbox_message(&app, &message_path, hooks.clone()).await;

        assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![exited_id]);
        let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
        assert_eq!(spawn_calls.len(), 1);
        assert!(!spawn_calls[0].skip_auto_resume);
        assert_spawn_call_matches_target(&spawn_calls[0], &fixture);

        let injected = hooks.inject_calls.lock().unwrap().clone();
        assert_eq!(injected.len(), 1);
        assert_ne!(injected[0], exited_id);
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        assert!(mgr.get_session(exited_id).await.is_none());
        drop(mgr);
        assert_spawned_session_matches_target(&app, injected[0], &fixture).await;
        assert_inject_results_consumed(&hooks);
    }

    #[tokio::test]
    async fn deliver_wake_respawns_exited_deferred_session_preserving_telegram_intent() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let exited_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "deferred",
            SessionStatus::Exited(0),
            Some("bot-1"),
        )
        .await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(exited_id, false);
        hooks.inject_results.lock().unwrap().push_back(Ok(()));
        let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-telegram");

        run_mailbox_message(&app, &message_path, hooks.clone()).await;

        assert_eq!(*hooks.destroy_calls.lock().unwrap(), vec![exited_id]);
        let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
        assert_eq!(spawn_calls.len(), 1);
        assert!(!spawn_calls[0].skip_auto_resume);
        assert_spawn_call_matches_target(&spawn_calls[0], &fixture);
        let injected = hooks.inject_calls.lock().unwrap().clone();
        assert_eq!(injected.len(), 1);
        let new_session_id = injected[0];
        assert_ne!(new_session_id, exited_id);
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        assert!(mgr.get_session(exited_id).await.is_none());
        drop(mgr);
        assert_spawned_session_matches_target(&app, new_session_id, &fixture).await;
        assert_eq!(
            *hooks.attach_calls.lock().unwrap(),
            vec![(new_session_id, Some("bot-1".into()))]
        );
        let events = hooks.events.lock().unwrap().clone();
        let attach_pos = events
            .iter()
            .position(|event| {
                *event
                    == MailboxTestEvent::Attach {
                        session_id: new_session_id,
                        bot_id: Some("bot-1".into()),
                    }
            })
            .unwrap();
        let inject_pos = events
            .iter()
            .position(|event| *event == MailboxTestEvent::Inject(new_session_id))
            .unwrap();
        assert!(attach_pos < inject_pos);
        assert_inject_results_consumed(&hooks);
    }

    #[tokio::test]
    async fn deliver_wake_promotes_resume_when_all_live_candidates_race_to_dead() {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let first_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "first",
            SessionStatus::Running,
            None,
        )
        .await;
        let second_id = add_mailbox_session(
            &app,
            &fixture.target_cwd,
            "second",
            SessionStatus::Running,
            None,
        )
        .await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(first_id, true);
        hooks.pty_presence.lock().unwrap().insert(second_id, true);
        {
            let mut results = hooks.inject_results.lock().unwrap();
            results.push_back(Err(format!(
                "PTY write failed: Session not found: {}",
                first_id
            )));
            results.push_back(Err(format!(
                "PTY write failed: Session not found: {}",
                second_id
            )));
            results.push_back(Ok(()));
        }
        let message_path = write_wake_outbox_message(&fixture.sender_cwd, "msg-promote");

        run_mailbox_message(&app, &message_path, hooks.clone()).await;

        assert!(hooks.destroy_calls.lock().unwrap().is_empty());
        let spawn_calls = hooks.spawn_calls.lock().unwrap().clone();
        assert_eq!(spawn_calls.len(), 1);
        assert!(!spawn_calls[0].skip_auto_resume);
        assert_spawn_call_matches_target(&spawn_calls[0], &fixture);
        let injected = hooks.inject_calls.lock().unwrap().clone();
        assert_eq!(injected.len(), 3);
        assert_eq!(&injected[0..2], &[first_id, second_id]);
        assert_ne!(injected[2], first_id);
        assert_ne!(injected[2], second_id);
        assert_spawned_session_matches_target(&app, injected[2], &fixture).await;
        assert_inject_results_consumed(&hooks);
    }

    // ── BOM-tolerant reader tests (issue #130) ──

    use std::io::Write;
    use tempfile::NamedTempFile;

    // Includes a non-BMP codepoint (😀 U+1F600 → surrogate pair D83D DE00) so the
    // UTF-16 LE/BE BOM tests exercise surrogate-pair decoding, not just BMP.
    const SAMPLE_JSON: &str = r#"{"id":"abc","kind":"ping","emoji":"😀"}"#;

    fn write_temp(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(bytes).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn bom_tolerant_reads_plain_utf8() {
        let f = write_temp(SAMPLE_JSON.as_bytes());
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        // Parses as JSON.
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_strips_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SAMPLE_JSON.as_bytes());
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_decodes_utf16_le_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in SAMPLE_JSON.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_decodes_utf16_be_bom() {
        let mut bytes = vec![0xFE, 0xFF];
        for u in SAMPLE_JSON.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_rejects_invalid_utf8_no_bom() {
        // Lone continuation byte 0x80 is invalid UTF-8 and not a recognized BOM.
        let f = write_temp(&[0x80, 0x81, 0x82]);
        let err = read_text_bom_tolerant(f.path()).expect_err("must err");
        assert!(err.contains("Invalid UTF-8"), "unexpected error: {}", err);
    }

    #[test]
    fn bom_tolerant_empty_file_returns_empty_string() {
        let f = write_temp(&[]);
        let got = read_text_bom_tolerant(f.path()).expect("empty ok");
        assert_eq!(got, "");
        // Serde fails with a parse error (NOT a read error) — confirms the failure
        // surfaces at the parser, which is what the callsites wrap with their own
        // context strings.
        let parse_err = serde_json::from_str::<serde_json::Value>(&got).expect_err("must err");
        assert!(
            parse_err.is_eof(),
            "expected EOF parse error, got: {}",
            parse_err
        );
    }

    /// §130-stuck-file regression: when the reject path receives a file whose
    /// bytes are non-UTF-8 and have no BOM (e.g. PowerShell `Set-Content
    /// -Encoding ANSI` from a CP1252 locale), `read_text_bom_tolerant` returns
    /// `Err` every poll cycle. Before this fix, the reject branch was guarded
    /// by `if let Ok(content) = ...` and dropped to `else { false }` on Err —
    /// the file stayed in the source dir at `attempt_count >= MAX`, looping
    /// forever. The new `Err(_) => reject_raw_file(...)` arm closes that gap.
    /// This test drives the fallback directly: an unreadable file is moved to
    /// `rejected/` and a reason file is written, exactly as the new branch does.
    #[test]
    fn reject_raw_file_moves_unreadable_outbox_file_to_rejected_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let outbox = tmp.path();
        let stuck = outbox.join("stuck.json");
        // CP1252 high-byte sequence — invalid UTF-8, no BOM.
        std::fs::write(&stuck, [0x80, 0x81, 0x82]).expect("write stuck file");

        // Precondition: this is exactly the input shape that hits the new Err arm.
        let read_err = read_text_bom_tolerant(&stuck).expect_err("read must err");
        assert!(
            read_err.contains("Invalid UTF-8"),
            "unexpected error: {}",
            read_err
        );

        // Drive the new fallback path.
        MailboxPoller::reject_raw_file(
            &stuck,
            "Undeliverable after 10 attempts. Last error: Failed to read outbox file: Invalid UTF-8",
        )
        .expect("reject_raw_file ok");

        assert!(
            !stuck.exists(),
            "original file should be moved out of source dir"
        );
        let rejected_dir = outbox.join("rejected");
        assert!(rejected_dir.is_dir(), "rejected/ should be created");
        assert!(
            rejected_dir.join("stuck.json").is_file(),
            "file should be moved to rejected/stuck.json"
        );
        let reason_file = rejected_dir.join("stuck.reason.txt");
        assert!(reason_file.is_file(), "reason file should be in rejected/");
        let reason = std::fs::read_to_string(&reason_file).expect("read reason");
        assert!(
            reason.contains("Undeliverable"),
            "reason content: {}",
            reason
        );
    }
}
