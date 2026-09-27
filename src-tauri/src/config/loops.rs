use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use uuid::Uuid;

use crate::config::ac_root::existing_ac_root;

pub const LOOP_DIR_PREFIX: &str = "_loop_";
pub const LOOP_CONFIG_FILE: &str = "config.toml";
pub const LOOP_STATE_FILE: &str = "state.json";
pub const LOOP_AUDIT_FILE: &str = "audit.jsonl";
pub const LOOP_TIMEZONE_LOCAL: &str = "local";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoopTriggerKind {
    Cron,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoopTargetKind {
    WorkgroupCoordinator,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MissedWhileClosedPolicy {
    #[default]
    Notify,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum BusyCoordinatorPolicy {
    #[default]
    WaitUntilIdle,
    ForceInject,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopSessionStart {
    Fresh,
    Accumulate,
}

// Explicit, not derived: Fresh is a product decision (a Loop starts a new
// conversation) and must read as one at the definition site.
#[allow(clippy::derivable_impls)]
impl Default for LoopSessionStart {
    fn default() -> Self {
        LoopSessionStart::Fresh
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfigToml {
    #[serde(rename = "loop")]
    pub loop_def: LoopDef,
    pub trigger: LoopTrigger,
    pub target: LoopTarget,
    pub prompt: LoopPrompt,
    #[serde(default)]
    pub policy: LoopPolicy,
}

#[derive(Debug, Clone, Default)]
pub struct LoopUpdatePatch {
    pub name: Option<String>,
    pub expr: Option<String>,
    pub workgroup: Option<String>,
    pub prompt_body: Option<String>,
    pub busy_coordinator: Option<BusyCoordinatorPolicy>,
    pub session_start: Option<LoopSessionStart>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDef {
    pub id: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopTrigger {
    pub kind: LoopTriggerKind,
    pub expr: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopTarget {
    pub kind: LoopTargetKind,
    pub workgroup: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopPrompt {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopPolicy {
    #[serde(default)]
    pub missed_while_closed: MissedWhileClosedPolicy,
    #[serde(default)]
    pub busy_coordinator: BusyCoordinatorPolicy,
    #[serde(default)]
    pub session_start: LoopSessionStart,
}

impl Default for LoopPolicy {
    fn default() -> Self {
        Self {
            missed_while_closed: MissedWhileClosedPolicy::Notify,
            busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
            session_start: LoopSessionStart::Fresh,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoopState {
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_due_at: Option<DateTime<Utc>>,
    pub last_delivered_at: Option<DateTime<Utc>>,
    pub last_result: Option<LoopLastResult>,
    pub pending_due_at: Option<DateTime<Utc>>,
    pub pending_run_id: Option<Uuid>,
    pub last_missed_closed_at: Option<DateTime<Utc>>,
    pub next_due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopLastResult {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LoopAuditKind {
    Delivered,
    PendingBusy,
    SkippedBusy,
    MissedWhileClosed,
    DeliveryFailed,
    CoalescedPending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopAuditEntry {
    pub run_id: Uuid,
    pub loop_id: String,
    pub project_path: String,
    pub kind: LoopAuditKind,
    pub due_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub target: Option<String>,
    pub session_id: Option<Uuid>,
    pub busy_coordinator_policy: BusyCoordinatorPolicy,
    pub session_start: Option<LoopSessionStart>,
    pub error: Option<String>,
    pub prompt_snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcLoopSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub expr: String,
    pub timezone: String,
    pub target_kind: LoopTargetKind,
    pub workgroup: String,
    pub prompt_preview: String,
    pub busy_coordinator: BusyCoordinatorPolicy,
    pub session_start: LoopSessionStart,
    pub path: String,
    pub config_path: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_due_at: Option<DateTime<Utc>>,
    pub last_delivered_at: Option<DateTime<Utc>>,
    pub last_result: Option<LoopLastResult>,
    pub pending_due_at: Option<DateTime<Utc>>,
    pub last_missed_closed_at: Option<DateTime<Utc>>,
    pub next_due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopConfigDetails {
    pub summary: AcLoopSummary,
    pub prompt_body: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedLoopTarget {
    pub target_fqn: String,
    pub project_dir: PathBuf,
    pub ac_root: PathBuf,
    pub wg_dir: PathBuf,
    pub coordinator_replica_dir: PathBuf,
    pub coordinator_agent_name: String,
}

fn default_enabled() -> bool {
    true
}

fn default_timezone() -> String {
    LOOP_TIMEZONE_LOCAL.to_string()
}

pub fn validate_loop_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Loop id cannot be empty".to_string());
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err("Loop id cannot start or end with a hyphen".to_string());
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(
            "Invalid Loop id: only alphanumeric characters and hyphens are allowed".to_string(),
        );
    }
    Ok(())
}

pub fn sanitize_loop_id(name: &str) -> Result<String, String> {
    let sanitized = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    validate_loop_id(&sanitized)?;
    Ok(sanitized)
}

pub fn loop_dir(ac_root: &Path, id: &str) -> PathBuf {
    ac_root.join(format!("{}{}", LOOP_DIR_PREFIX, id))
}

pub fn read_loop_config(loop_dir: &Path) -> Result<LoopConfigToml, String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
    parse_loop_config(&config_path, &content)
}

/// Like `read_loop_config`, but a missing Loop directory or `config.toml`
/// is `Ok(None)` instead of a read error. A malformed file is still an error.
pub fn read_loop_config_if_present(loop_dir: &Path) -> Result<Option<LoopConfigToml>, String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    match std::fs::read_to_string(&config_path) {
        Ok(content) => parse_loop_config(&config_path, &content).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read {}: {}", config_path.display(), e)),
    }
}

fn parse_loop_config(config_path: &Path, content: &str) -> Result<LoopConfigToml, String> {
    toml::from_str(content).map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopConfigRevalidation {
    Current,
    Gone,
    Disabled,
    Changed,
}

pub fn revalidate_loop_current(
    loop_dir: &Path,
    expected: &LoopConfigToml,
) -> Result<LoopConfigRevalidation, String> {
    if !loop_dir.is_dir() || !loop_dir.join(LOOP_CONFIG_FILE).is_file() {
        return Ok(LoopConfigRevalidation::Gone);
    }
    let current = read_loop_config(loop_dir)?;
    if !current.loop_def.enabled {
        return Ok(LoopConfigRevalidation::Disabled);
    }
    if loop_delivery_config_matches(&current, expected) {
        Ok(LoopConfigRevalidation::Current)
    } else {
        Ok(LoopConfigRevalidation::Changed)
    }
}

pub fn loop_delivery_config_matches(current: &LoopConfigToml, expected: &LoopConfigToml) -> bool {
    current.loop_def.id == expected.loop_def.id
        && current.loop_def.enabled == expected.loop_def.enabled
        && current.trigger.kind == expected.trigger.kind
        && current.trigger.expr == expected.trigger.expr
        && current.trigger.timezone == expected.trigger.timezone
        && current.target.kind == expected.target.kind
        && current.target.workgroup == expected.target.workgroup
        && current.prompt.body == expected.prompt.body
        && current.policy.missed_while_closed == expected.policy.missed_while_closed
        && current.policy.busy_coordinator == expected.policy.busy_coordinator
        && current.policy.session_start == expected.policy.session_start
}

pub fn write_loop_config(ac_root: &Path, config: &LoopConfigToml) -> Result<PathBuf, String> {
    validate_loop_id(&config.loop_def.id)?;
    let dir = loop_dir(ac_root, &config.loop_def.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create Loop directory: {}", e))?;
    let content = toml::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize Loop config: {}", e))?;
    // Write a sibling tmp file and replace, so no reader in any process
    // (including the CLI) ever sees a torn config.toml.
    let config_path = dir.join(LOOP_CONFIG_FILE);
    let tmp_path = dir.join(format!("{}.{}.tmp", LOOP_CONFIG_FILE, Uuid::new_v4()));
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write Loop config: {}", e))?;
    replace_file_with_retry(&tmp_path, &config_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to write Loop config: {}", e)
    })?;
    Ok(dir)
}

pub fn read_loop_state(loop_dir: &Path) -> Result<LoopState, String> {
    read_loop_state_with_raw(loop_dir).map(|(state, _)| state)
}

/// Reads `state.json` and also returns its raw text (`None` when absent), the
/// expectation `write_loop_state_if_unchanged` compares against.
pub fn read_loop_state_with_raw(loop_dir: &Path) -> Result<(LoopState, Option<String>), String> {
    let state_path = loop_dir.join(LOOP_STATE_FILE);
    if !state_path.exists() {
        return Ok((LoopState::default(), None));
    }
    let content = std::fs::read_to_string(&state_path)
        .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
    let state = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))?;
    Ok((state, Some(content)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStateWrite {
    Written,
    Stale,
}

/// Compare-and-swap on `state.json`: writes only when the on-disk raw bytes
/// still equal `expected_raw` (absent included). Not a lock by itself; it is
/// atomic only for a caller that holds a lock across the read and this call.
pub fn write_loop_state_if_unchanged(
    loop_dir: &Path,
    state: &LoopState,
    expected_raw: Option<&str>,
) -> Result<LoopStateWrite, String> {
    let state_path = loop_dir.join(LOOP_STATE_FILE);
    let current_raw = match std::fs::read_to_string(&state_path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Failed to read {}: {}", state_path.display(), e)),
    };
    if current_raw.as_deref() != expected_raw {
        return Ok(LoopStateWrite::Stale);
    }
    write_loop_state_atomic(loop_dir, state)?;
    Ok(LoopStateWrite::Written)
}

pub fn write_loop_state_atomic(loop_dir: &Path, state: &LoopState) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let state_path = loop_dir.join(LOOP_STATE_FILE);
    let tmp_path = loop_dir.join(format!("{}.{}.tmp", LOOP_STATE_FILE, Uuid::new_v4()));
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize Loop state: {}", e))?;
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temporary Loop state: {}", e))?;
    replace_file_with_retry(&tmp_path, &state_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize Loop state: {}", e)
    })
}

/// Sleeps between replace attempts: 5 attempts, at most 150 ms added.
const REPLACE_RETRY_DELAYS_MS: [u64; 4] = [10, 20, 40, 80];

/// `replace_file`, retried while the destination is transiently held open
/// (any open handle, even a fully shared reader, fails `MoveFileExW`).
/// Blocking `std::thread::sleep` on purpose: the callers are synchronous and
/// the CLI uses them too; 150 ms on an already-failing path is cheaper than
/// making the write path async.
fn replace_file_with_retry(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut delays = REPLACE_RETRY_DELAYS_MS.iter();
    loop {
        match replace_file(src, dst) {
            Err(e) if is_transient_replace_error(&e) => match delays.next() {
                Some(ms) => std::thread::sleep(std::time::Duration::from_millis(*ms)),
                None => return Err(e),
            },
            result => return result,
        }
    }
}

#[cfg(windows)]
fn is_transient_replace_error(e: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED (what a held destination produces) and
    // ERROR_SHARING_VIOLATION.
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
fn is_transient_replace_error(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}

pub fn append_loop_audit_once(loop_dir: &Path, entry: &LoopAuditEntry) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let audit_path = loop_dir.join(LOOP_AUDIT_FILE);
    if audit_path.exists() {
        let content = std::fs::read_to_string(&audit_path)
            .map_err(|e| format!("Failed to read {}: {}", audit_path.display(), e))?;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<LoopAuditEntry>(line) {
                Ok(existing)
                    if existing.loop_id == entry.loop_id
                        && existing.run_id == entry.run_id
                        && existing.kind == entry.kind
                        && existing.due_at == entry.due_at =>
                {
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => log::warn!(
                    "[loops] Ignoring malformed audit line in {}: {}",
                    audit_path.display(),
                    e
                ),
            }
        }
    }

    let line = serde_json::to_string(entry)
        .map_err(|e| format!("Failed to serialize Loop audit entry: {}", e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)
        .map_err(|e| format!("Failed to open {}: {}", audit_path.display(), e))?;
    writeln!(file, "{}", line)
        .map_err(|e| format!("Failed to append {}: {}", audit_path.display(), e))
}

fn ensure_loop_config_present(loop_dir: &Path) -> Result<(), String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    if config_path.is_file() {
        Ok(())
    } else {
        Err(format!(
            "Loop config no longer exists at {}",
            config_path.display()
        ))
    }
}

pub fn discover_loops_in_project(project_dir: &Path) -> Vec<AcLoopSummary> {
    let Some(ac_root) = existing_ac_root(project_dir) else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(&ac_root) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!(
                "[loops] Failed to read Project AC Root {} for Loop discovery: {}",
                ac_root.display(),
                e
            );
            return Vec::new();
        }
    };

    let mut loops = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(LOOP_DIR_PREFIX) {
            continue;
        }
        let config = match read_loop_config(&dir) {
            Ok(config) => config,
            Err(e) => {
                log::warn!(
                    "[loops] Skipping malformed Loop at {}: {}",
                    dir.display(),
                    e
                );
                continue;
            }
        };
        let state = match read_loop_state(&dir) {
            Ok(state) => state,
            Err(e) => {
                log::warn!(
                    "[loops] Ignoring malformed state for Loop '{}' at {}: {}",
                    config.loop_def.id,
                    dir.display(),
                    e
                );
                LoopState::default()
            }
        };
        loops.push(summary_from_parts(&dir, &config, &state));
    }
    loops.sort_by_key(|item| item.name.to_lowercase());
    loops
}

pub fn validate_loop_config(project_dir: &Path, config: &LoopConfigToml) -> Result<(), String> {
    validate_loop_id(&config.loop_def.id)?;
    if config.loop_def.name.trim().is_empty() {
        return Err("Loop name cannot be empty".to_string());
    }
    if config.trigger.timezone != LOOP_TIMEZONE_LOCAL {
        return Err("Loop timezone must be 'local' for the MVP".to_string());
    }
    validate_cron_expr(&config.trigger.expr)?;
    if !crate::config::entity_prefix::has_entity_prefix(&config.target.workgroup) {
        return Err(
            "Loop target room must be a `room-*` or legacy `wg-*` Room directory name".to_string(),
        );
    }
    validate_workgroup_name(&config.target.workgroup)?;
    if config.prompt.body.trim().is_empty() {
        return Err("Loop prompt cannot be empty".to_string());
    }
    resolve_loop_target(project_dir, config)?;
    Ok(())
}

pub fn resolve_loop_target(
    project_dir: &Path,
    config: &LoopConfigToml,
) -> Result<ResolvedLoopTarget, String> {
    let ac_root = existing_ac_root(project_dir).ok_or_else(|| {
        format!(
            "Project AC Root not found in {} (.ac)",
            project_dir.display()
        )
    })?;
    let wg_dir = ac_root.join(&config.target.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!(
            "Room '{}' not found in project {}",
            config.target.workgroup,
            project_dir.display()
        ));
    }
    let resolved = crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &wg_dir)
        .ok_or_else(|| {
            format!(
                "Room '{}' has no identity-verified orchestrator",
                config.target.workgroup
            )
        })?;
    let target_fqn = format!(
        "{}:{}/{}",
        resolved.project, resolved.wg_name, resolved.agent_name
    );
    Ok(ResolvedLoopTarget {
        target_fqn,
        project_dir: project_dir.to_path_buf(),
        ac_root,
        wg_dir,
        coordinator_replica_dir: resolved.replica_dir,
        coordinator_agent_name: resolved.agent_name,
    })
}

pub fn validate_cron_expr(expr: &str) -> Result<(), String> {
    if expr.split_whitespace().count() != 5 {
        return Err(
            "Cron expression must use exactly 5 fields: minute hour day-of-month month day-of-week"
                .to_string(),
        );
    }
    croner::Cron::from_str(expr)
        .map(|_| ())
        .map_err(|e| format!("Invalid cron expression: {}", e))
}

pub fn next_due_after(expr: &str, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
    validate_cron_expr(expr)?;
    let cron =
        croner::Cron::from_str(expr).map_err(|e| format!("Invalid cron expression: {}", e))?;
    let local_after = after.with_timezone(&Local);
    cron.find_next_occurrence(&local_after, false)
        .map(|next| Some(next.with_timezone(&Utc)))
        .map_err(|e| format!("Failed to calculate next Loop due time: {}", e))
}

pub fn latest_due_between(
    expr: &str,
    after: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    validate_cron_expr(expr)?;
    if after >= now {
        return Ok(None);
    }
    let cron =
        croner::Cron::from_str(expr).map_err(|e| format!("Invalid cron expression: {}", e))?;
    let now_local = now.with_timezone(&Local);
    let latest = cron
        .find_previous_occurrence(&now_local, true)
        .map_err(|e| format!("Failed to calculate Loop due time: {}", e))?
        .with_timezone(&Utc);
    if latest > after && latest <= now {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

pub fn baseline_loop_state(
    config: &LoopConfigToml,
    now: DateTime<Utc>,
) -> Result<LoopState, String> {
    Ok(LoopState {
        last_checked_at: Some(now),
        next_due_at: next_due_after(&config.trigger.expr, now)?,
        ..LoopState::default()
    })
}

pub fn apply_loop_update_patch(
    config: &mut LoopConfigToml,
    patch: LoopUpdatePatch,
) -> Result<bool, String> {
    let mut reset_schedule = false;

    if let Some(name) = patch.name {
        if name.trim().is_empty() {
            return Err("Loop name cannot be empty".to_string());
        }
        config.loop_def.name = name;
    }
    if let Some(expr) = patch.expr {
        if config.trigger.expr != expr {
            config.trigger.expr = expr;
            reset_schedule = true;
        }
    }
    if let Some(workgroup) = patch.workgroup {
        if config.target.workgroup != workgroup {
            config.target.workgroup = workgroup;
            reset_schedule = true;
        }
    }
    if let Some(prompt_body) = patch.prompt_body {
        if prompt_body.trim().is_empty() {
            return Err("Loop prompt cannot be empty".to_string());
        }
        if config.prompt.body != prompt_body {
            config.prompt.body = prompt_body;
            reset_schedule = true;
        }
    }
    if let Some(policy) = patch.busy_coordinator {
        if config.policy.busy_coordinator != policy {
            config.policy.busy_coordinator = policy;
            reset_schedule = true;
        }
    }
    if let Some(session_start) = patch.session_start {
        if config.policy.session_start != session_start {
            config.policy.session_start = session_start;
            reset_schedule = true;
        }
    }
    if let Some(enabled) = patch.enabled {
        if config.loop_def.enabled != enabled {
            config.loop_def.enabled = enabled;
            reset_schedule = true;
        }
    }

    Ok(reset_schedule)
}

pub fn summary_from_parts(dir: &Path, config: &LoopConfigToml, state: &LoopState) -> AcLoopSummary {
    AcLoopSummary {
        id: config.loop_def.id.clone(),
        name: config.loop_def.name.clone(),
        enabled: config.loop_def.enabled,
        expr: config.trigger.expr.clone(),
        timezone: config.trigger.timezone.clone(),
        target_kind: config.target.kind.clone(),
        workgroup: config.target.workgroup.clone(),
        prompt_preview: prompt_preview(&config.prompt.body),
        busy_coordinator: config.policy.busy_coordinator.clone(),
        session_start: config.policy.session_start,
        path: dir.to_string_lossy().to_string(),
        config_path: dir.join(LOOP_CONFIG_FILE).to_string_lossy().to_string(),
        last_checked_at: state.last_checked_at,
        last_due_at: state.last_due_at,
        last_delivered_at: state.last_delivered_at,
        last_result: state.last_result.clone(),
        pending_due_at: state.pending_due_at,
        last_missed_closed_at: state.last_missed_closed_at,
        next_due_at: state.next_due_at,
    }
}

pub fn details_from_parts(
    dir: &Path,
    config: &LoopConfigToml,
    state: &LoopState,
) -> LoopConfigDetails {
    LoopConfigDetails {
        summary: summary_from_parts(dir, config, state),
        prompt_body: config.prompt.body.clone(),
    }
}

pub fn prompt_preview(body: &str) -> String {
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    const LIMIT: usize = 160;
    if normalized.chars().count() <= LIMIT {
        return normalized;
    }
    let mut preview = normalized.chars().take(LIMIT - 3).collect::<String>();
    preview.push_str("...");
    preview
}

fn validate_workgroup_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Room name cannot be empty".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(
            "Invalid Room name: only alphanumeric characters and hyphens are allowed".to_string(),
        );
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(src, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let matrix = ac_root.join("_agent_tech-lead");
        let wg = ac_root.join("wg-1-dev-team");
        let replica = wg.join("__agent_tech-lead");
        for dir in [&team_dir, &matrix, &replica] {
            std::fs::create_dir_all(dir).expect("create fixture dir");
        }
        std::fs::write(matrix.join("Role.md"), "# Tech Lead\n").expect("role");
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["_agent_tech-lead"],"coordinator":"_agent_tech-lead","repos":[]}"#,
        )
        .expect("team config");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .expect("replica config");
        tmp
    }

    fn sample_config() -> LoopConfigToml {
        LoopConfigToml {
            loop_def: LoopDef {
                id: "weekday-standup".to_string(),
                name: "Weekday standup".to_string(),
                enabled: true,
            },
            trigger: LoopTrigger {
                kind: LoopTriggerKind::Cron,
                expr: "0 9 * * 1-5".to_string(),
                timezone: LOOP_TIMEZONE_LOCAL.to_string(),
            },
            target: LoopTarget {
                kind: LoopTargetKind::WorkgroupCoordinator,
                workgroup: "wg-1-dev-team".to_string(),
            },
            prompt: LoopPrompt {
                body: "Summarize status".to_string(),
            },
            policy: LoopPolicy::default(),
        }
    }

    #[test]
    fn loop_id_sanitizes_and_rejects_unsafe_existing_ids() {
        assert_eq!(
            sanitize_loop_id("Weekday Standup!").unwrap(),
            "weekday-standup"
        );
        for id in ["", "-daily", "daily-", "../daily", "daily!", "daily_loop"] {
            assert!(validate_loop_id(id).is_err(), "{id} should fail");
        }
    }

    #[test]
    fn loop_policy_serializes_camel_case_keys() {
        let mut config = sample_config();
        config.policy.busy_coordinator = BusyCoordinatorPolicy::ForceInject;
        let toml = toml::to_string(&config).expect("toml");
        assert!(toml.contains("missedWhileClosed = \"notify\""), "{toml}");
        assert!(toml.contains("busyCoordinator = \"forceInject\""), "{toml}");
    }

    #[test]
    fn cron_validation_rejects_six_fields_before_parser() {
        let err = validate_cron_expr("0 0 9 * * 1").unwrap_err();
        assert!(err.contains("exactly 5 fields"), "{err}");
    }

    #[test]
    fn cron_preview_returns_future_due_time() {
        let after = "2026-06-13T21:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("time");
        let next = next_due_after("*/5 * * * *", after)
            .expect("preview")
            .expect("next");
        assert!(next > after);
    }

    #[test]
    fn latest_due_between_uses_previous_match_for_long_gaps() {
        let now = "2026-06-13T21:17:00Z"
            .parse::<DateTime<Utc>>()
            .expect("time");
        let after = now - chrono::Duration::days(30);
        let due = latest_due_between("* * * * *", after, now)
            .expect("due")
            .expect("due present");

        assert!(due > after);
        assert!(due <= now);
        assert!(now - due < chrono::Duration::minutes(2));
    }

    #[test]
    fn loop_update_patch_only_resets_on_actual_schedule_or_delivery_changes() {
        let mut config = sample_config();
        let reset = apply_loop_update_patch(
            &mut config,
            LoopUpdatePatch {
                name: Some("Renamed standup".to_string()),
                ..LoopUpdatePatch::default()
            },
        )
        .expect("name update");
        assert!(!reset);
        assert_eq!(config.loop_def.name, "Renamed standup");

        let reset = apply_loop_update_patch(
            &mut config,
            LoopUpdatePatch {
                expr: Some("0 9 * * 1-5".to_string()),
                workgroup: Some("wg-1-dev-team".to_string()),
                prompt_body: Some("Summarize status".to_string()),
                busy_coordinator: Some(BusyCoordinatorPolicy::WaitUntilIdle),
                enabled: Some(true),
                ..LoopUpdatePatch::default()
            },
        )
        .expect("noop update");
        assert!(!reset);

        let reset = apply_loop_update_patch(
            &mut config,
            LoopUpdatePatch {
                expr: Some("30 9 * * 1-5".to_string()),
                ..LoopUpdatePatch::default()
            },
        )
        .expect("cron update");
        assert!(reset);
        assert_eq!(config.trigger.expr, "30 9 * * 1-5");
    }

    /// AC-1 - a legacy persisted config (no `sessionStart` key) loads as
    /// `Fresh`; the fixture is asserted not to carry the key so the test cannot
    /// pass for the wrong reason.
    #[test]
    fn legacy_config_without_session_start_loads_fresh() {
        let fixture = r#"
[loop]
id = "legacy-loop"
name = "Legacy loop"
enabled = true

[trigger]
kind = "cron"
expr = "0 9 * * *"
timezone = "local"

[target]
kind = "workgroupCoordinator"
workgroup = "wg-1-dev-team"

[prompt]
body = "Send status"

[policy]
busyCoordinator = "waitUntilIdle"
"#;
        assert!(!fixture.contains("sessionStart"));

        let parsed: LoopConfigToml = toml::from_str(fixture).expect("legacy config parses");
        assert_eq!(parsed.policy.session_start, LoopSessionStart::Fresh);
        assert_eq!(
            parsed.policy.busy_coordinator,
            BusyCoordinatorPolicy::WaitUntilIdle
        );
    }

    /// AC-2 - the struct default is `Fresh` and both variants keep their wire
    /// spellings across a serialize/deserialize round trip.
    #[test]
    fn loop_session_start_defaults_to_fresh_and_round_trips() {
        assert_eq!(LoopPolicy::default().session_start, LoopSessionStart::Fresh);
        for (variant, wire) in [
            (LoopSessionStart::Fresh, "\"fresh\""),
            (LoopSessionStart::Accumulate, "\"accumulate\""),
        ] {
            let encoded = serde_json::to_string(&variant).expect("serialize session start");
            assert_eq!(encoded, wire);
            let decoded: LoopSessionStart =
                serde_json::from_str(&encoded).expect("deserialize session start");
            assert_eq!(decoded, variant);
        }

        let toml = toml::to_string(&sample_config()).expect("toml");
        assert!(toml.contains("sessionStart = \"fresh\""), "{toml}");
    }

    /// AC-11 - `None` leaves the stored value untouched; `Some(Accumulate)`
    /// applies exactly like `busy_coordinator` and the summary reflects it.
    #[test]
    fn loop_update_patch_applies_session_start_like_busy_coordinator() {
        let mut config = sample_config();
        assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);

        let reset = apply_loop_update_patch(
            &mut config,
            LoopUpdatePatch {
                session_start: None,
                ..LoopUpdatePatch::default()
            },
        )
        .expect("no-op patch");
        assert!(!reset);
        assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);

        let reset = apply_loop_update_patch(
            &mut config,
            LoopUpdatePatch {
                session_start: Some(LoopSessionStart::Accumulate),
                ..LoopUpdatePatch::default()
            },
        )
        .expect("session start patch");
        assert!(reset);
        assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);

        let tmp = fixture_project();
        let summary = summary_from_parts(&tmp.path().join(".ac"), &config, &LoopState::default());
        assert_eq!(summary.session_start, LoopSessionStart::Accumulate);
    }

    /// AC-14 - a `sessionStart`-only edit is visible to the delivery
    /// revalidation, and equal configs still match.
    #[test]
    fn loop_delivery_config_matches_pins_session_start() {
        let current = sample_config();
        assert!(loop_delivery_config_matches(&current, &current.clone()));

        let mut expected = current.clone();
        expected.policy.session_start = LoopSessionStart::Accumulate;
        assert!(!loop_delivery_config_matches(&current, &expected));
        assert!(!loop_delivery_config_matches(&expected, &current));
    }

    #[test]
    fn storage_writes_config_state_and_dedupes_audit() {
        let tmp = fixture_project();
        let ac_root = tmp.path().join(".ac");
        let config = sample_config();
        validate_loop_config(tmp.path(), &config).expect("valid config");

        let dir = write_loop_config(&ac_root, &config).expect("write config");
        assert!(dir.join(LOOP_CONFIG_FILE).is_file());

        let state = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };
        write_loop_state_atomic(&dir, &state).expect("state write");
        let read = read_loop_state(&dir).expect("state read");
        assert!(read.last_checked_at.is_some());

        let entry = LoopAuditEntry {
            run_id: Uuid::new_v4(),
            loop_id: config.loop_def.id.clone(),
            project_path: tmp.path().to_string_lossy().to_string(),
            kind: LoopAuditKind::PendingBusy,
            due_at: Utc::now(),
            started_at: Utc::now(),
            completed_at: None,
            target: Some("proj:wg-1-dev-team/tech-lead".to_string()),
            session_id: None,
            busy_coordinator_policy: BusyCoordinatorPolicy::WaitUntilIdle,
            session_start: Some(LoopSessionStart::Fresh),
            error: None,
            prompt_snapshot: None,
        };
        append_loop_audit_once(&dir, &entry).expect("audit one");
        append_loop_audit_once(&dir, &entry).expect("audit duplicate");
        let content = std::fs::read_to_string(dir.join(LOOP_AUDIT_FILE)).expect("audit read");
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn storage_writes_do_not_recreate_loop_dirs_without_config() {
        let tmp = fixture_project();
        let ac_root = tmp.path().join(".ac");
        let config = sample_config();
        let dir = write_loop_config(&ac_root, &config).expect("write config");
        let state = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };
        let entry = LoopAuditEntry {
            run_id: Uuid::new_v4(),
            loop_id: config.loop_def.id.clone(),
            project_path: tmp.path().to_string_lossy().to_string(),
            kind: LoopAuditKind::PendingBusy,
            due_at: Utc::now(),
            started_at: Utc::now(),
            completed_at: None,
            target: Some("proj:wg-1-dev-team/tech-lead".to_string()),
            session_id: None,
            busy_coordinator_policy: BusyCoordinatorPolicy::WaitUntilIdle,
            session_start: Some(LoopSessionStart::Fresh),
            error: None,
            prompt_snapshot: None,
        };

        std::fs::remove_dir_all(&dir).expect("remove loop dir");
        assert!(write_loop_state_atomic(&dir, &state).is_err());
        assert!(!dir.exists());
        assert!(append_loop_audit_once(&dir, &entry).is_err());
        assert!(!dir.exists());

        std::fs::create_dir_all(&dir).expect("recreate dir without config");
        assert!(write_loop_state_atomic(&dir, &state).is_err());
        assert!(append_loop_audit_once(&dir, &entry).is_err());
        assert!(!dir.join(LOOP_STATE_FILE).exists());
        assert!(!dir.join(LOOP_AUDIT_FILE).exists());
    }

    #[test]
    fn discovery_omits_prompt_body_and_tolerates_bad_state() {
        let tmp = fixture_project();
        let ac_root = tmp.path().join(".ac");
        let config = sample_config();
        let dir = write_loop_config(&ac_root, &config).expect("write config");
        std::fs::write(dir.join(LOOP_STATE_FILE), "{bad json").expect("bad state");

        let loops = discover_loops_in_project(tmp.path());
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].id, "weekday-standup");
        assert_eq!(loops[0].prompt_preview, "Summarize status");
        assert!(loops[0].last_checked_at.is_none());
    }

    /// AC-15 - a legacy audit row (written before `sessionStart` existed)
    /// parses, and re-serializes the field as JSON `null` rather than as any
    /// concrete value the run may never have used. The test names no Rust
    /// field, so it compiles against both shapes and fails at runtime.
    #[test]
    fn legacy_audit_row_without_session_start_parses_as_not_recorded() {
        let line = r#"{"runId":"6a7cfa8e-0e0a-4a0f-9d1e-2f3d9a1b4c55","loopId":"daily-sync","projectPath":"/tmp/project","kind":"pendingBusy","dueAt":"2025-01-01T09:00:00Z","startedAt":"2025-01-01T09:00:01Z","completedAt":null,"target":"proj:wg-1-dev-team/tech-lead","sessionId":null,"busyCoordinatorPolicy":"waitUntilIdle","error":null,"promptSnapshot":null}"#;

        assert!(
            !line.contains("sessionStart"),
            "the legacy fixture must not carry the key, or this test passes for the wrong reason"
        );

        let parsed = serde_json::from_str::<LoopAuditEntry>(line).expect("legacy audit row parses");

        assert_eq!(
            serde_json::to_value(&parsed)
                .expect("re-serialize")
                .get("sessionStart"),
            Some(&serde_json::Value::Null),
            "an absent key must read back as not recorded, never as a concrete default"
        );
    }

    /// AC-16 - a legacy audit row still takes part in the append-once dedupe,
    /// so no duplicate row is appended and the file is never rewritten.
    #[test]
    fn legacy_audit_row_still_deduplicates_an_append() {
        let tmp = fixture_project();
        let ac_root = tmp.path().join(".ac");
        let config = sample_config();
        let dir = write_loop_config(&ac_root, &config).expect("write config");

        let legacy = format!(
            r#"{{"runId":"6a7cfa8e-0e0a-4a0f-9d1e-2f3d9a1b4c55","loopId":"{}","projectPath":"/tmp/project","kind":"pendingBusy","dueAt":"2025-01-01T09:00:00Z","startedAt":"2025-01-01T09:00:01Z","completedAt":null,"target":null,"sessionId":null,"busyCoordinatorPolicy":"waitUntilIdle","error":null,"promptSnapshot":null}}"#,
            config.loop_def.id
        );
        assert!(!legacy.contains("sessionStart"));
        let audit_path = dir.join(LOOP_AUDIT_FILE);
        std::fs::write(&audit_path, format!("{}\n", legacy)).expect("write legacy audit line");

        let mut value: serde_json::Value =
            serde_json::from_str(&legacy).expect("legacy line as value");
        value
            .as_object_mut()
            .expect("object")
            .insert("sessionStart".to_string(), serde_json::json!("fresh"));
        let entry: LoopAuditEntry = serde_json::from_value(value).expect("entry to append");

        append_loop_audit_once(&dir, &entry).expect("append against a legacy audit file");

        let content = std::fs::read_to_string(&audit_path).expect("audit read");
        assert_eq!(
            content.lines().count(),
            1,
            "the legacy row must be seen by the dedupe, so nothing is appended"
        );
        assert_eq!(content, format!("{}\n", legacy), "no row may be rewritten");
    }

    fn tmp_files_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("read loop dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    /// Opens `path` with the default (fully shared) OpenOptions and keeps the
    /// handle for `hold`, from another thread.
    #[cfg(windows)]
    fn hold_open_for(path: &Path, hold: std::time::Duration) -> std::thread::JoinHandle<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .expect("open holder");
        std::thread::spawn(move || {
            std::thread::sleep(hold);
            drop(file);
        })
    }

    /// T1 - every patch field that resets the schedule is exactly a field the
    /// delivery comparison sees. The exhaustive destructure makes a new patch
    /// field a compile error here.
    #[test]
    fn t1_patch_reset_fields_match_delivery_comparison() {
        let base = sample_config();
        let LoopUpdatePatch {
            name,
            expr,
            workgroup,
            prompt_body,
            busy_coordinator,
            session_start,
            enabled,
        } = LoopUpdatePatch {
            name: Some("Other name".to_string()),
            expr: Some("30 9 * * 1-5".to_string()),
            workgroup: Some("wg-2-dev-team".to_string()),
            prompt_body: Some("Other prompt".to_string()),
            busy_coordinator: Some(BusyCoordinatorPolicy::ForceInject),
            session_start: Some(LoopSessionStart::Accumulate),
            enabled: Some(false),
        };
        let patches = [
            (
                "name",
                LoopUpdatePatch {
                    name,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "expr",
                LoopUpdatePatch {
                    expr,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "workgroup",
                LoopUpdatePatch {
                    workgroup,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "prompt_body",
                LoopUpdatePatch {
                    prompt_body,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "busy_coordinator",
                LoopUpdatePatch {
                    busy_coordinator,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "session_start",
                LoopUpdatePatch {
                    session_start,
                    ..LoopUpdatePatch::default()
                },
            ),
            (
                "enabled",
                LoopUpdatePatch {
                    enabled,
                    ..LoopUpdatePatch::default()
                },
            ),
        ];
        for (field, patch) in patches {
            let mut patched = base.clone();
            let reset = apply_loop_update_patch(&mut patched, patch).expect("patch applies");
            let matches = loop_delivery_config_matches(&base, &patched);
            assert_eq!(
                reset, !matches,
                "{field}: reset_schedule must equal !matches"
            );
            if field == "name" {
                assert!(!reset && matches, "name must neither reset nor differ");
            } else {
                assert!(reset, "{field} must reset the schedule");
            }
        }
    }

    /// T4 - a real holder on state.json: two positive controls, then the
    /// retry succeeding and giving up.
    #[cfg(windows)]
    #[test]
    fn t4_replace_retry_under_a_real_holder() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let tmp = fixture_project();
        let dir = write_loop_config(&tmp.path().join(".ac"), &sample_config()).expect("config");
        let state_path = dir.join(LOOP_STATE_FILE);
        write_loop_state_atomic(&dir, &LoopState::default()).expect("initial state");

        for (label, share) in [
            ("default", None),
            ("FILE_SHARE_READ", Some(FILE_SHARE_READ)),
        ] {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            if let Some(mode) = share {
                options.share_mode(mode);
            }
            let holder = options.open(&state_path).expect("holder");
            let src = dir.join("control.tmp");
            std::fs::write(&src, "{}").expect("control tmp");
            let err = replace_file(&src, &state_path)
                .expect_err("positive control: a held destination must fail the replace");
            println!(
                "T4 positive control ({label}): {err} raw={:?}",
                err.raw_os_error()
            );
            assert_eq!(err.raw_os_error(), Some(5), "{label} holder");
            drop(holder);
            std::fs::remove_file(&src).expect("remove control tmp");
        }

        let next = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };
        let releaser = hold_open_for(&state_path, Duration::from_millis(40));
        let started = Instant::now();
        write_loop_state_atomic(&dir, &next).expect("retry succeeds after a 40 ms hold");
        println!("T4 success leg: Ok after {:?}", started.elapsed());
        releaser.join().expect("releaser");
        let (read_back, _) = read_loop_state_with_raw(&dir).expect("state parses");
        assert_eq!(read_back.last_checked_at, next.last_checked_at);

        let holder = std::fs::OpenOptions::new()
            .read(true)
            .open(&state_path)
            .expect("long holder");
        let started = Instant::now();
        let err = write_loop_state_atomic(&dir, &LoopState::default()).expect_err("gives up");
        let elapsed = started.elapsed();
        drop(holder);
        println!("T4 give-up leg: {err} after {elapsed:?}");
        assert!(err.starts_with("Failed to finalize Loop state"), "{err}");
        assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
        assert!(tmp_files_in(&dir).is_empty(), "{:?}", tmp_files_in(&dir));
    }

    /// T5 - the transient classifier.
    #[test]
    fn t5_transient_replace_error_classifier() {
        #[cfg(windows)]
        {
            assert!(is_transient_replace_error(
                &std::io::Error::from_raw_os_error(5)
            ));
            assert!(is_transient_replace_error(
                &std::io::Error::from_raw_os_error(32)
            ));
        }
        #[cfg(not(windows))]
        assert!(is_transient_replace_error(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        assert!(!is_transient_replace_error(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
    }

    /// T6 - config writes are atomic and leave no tmp file.
    #[test]
    fn t6_atomic_config_write_leaves_no_tmp() {
        let tmp = fixture_project();
        let ac_root = tmp.path().join(".ac");
        let mut config = sample_config();
        let dir = write_loop_config(&ac_root, &config).expect("first write");
        assert!(dir.join(LOOP_CONFIG_FILE).is_file());
        assert!(tmp_files_in(&dir).is_empty());
        assert_eq!(
            toml::to_string(&read_loop_config(&dir).expect("read")).expect("toml"),
            toml::to_string(&config).expect("toml")
        );

        config.loop_def.name = "Second write".to_string();
        write_loop_config(&ac_root, &config).expect("second write");
        assert!(tmp_files_in(&dir).is_empty());
        assert_eq!(
            read_loop_config(&dir).expect("read").loop_def.name,
            "Second write"
        );

        #[cfg(windows)]
        {
            let releaser = hold_open_for(
                &dir.join(LOOP_CONFIG_FILE),
                std::time::Duration::from_millis(40),
            );
            config.loop_def.name = "Held write".to_string();
            write_loop_config(&ac_root, &config).expect("retry covers the config path");
            releaser.join().expect("releaser");
            assert_eq!(
                read_loop_config(&dir).expect("read").loop_def.name,
                "Held write"
            );
            assert!(tmp_files_in(&dir).is_empty());
        }
    }

    #[test]
    fn read_loop_config_if_present_distinguishes_absent_from_malformed() {
        let tmp = fixture_project();
        let dir = tmp.path().join(".ac").join("loop-missing");
        assert!(read_loop_config_if_present(&dir)
            .expect("absent dir")
            .is_none());
        std::fs::create_dir_all(&dir).expect("dir");
        assert!(read_loop_config_if_present(&dir)
            .expect("absent file")
            .is_none());
        std::fs::write(dir.join(LOOP_CONFIG_FILE), "not = [toml").expect("bad");
        let err = read_loop_config_if_present(&dir).expect_err("malformed");
        assert!(err.starts_with("Failed to parse"), "{err}");
    }

    /// T11 - the compare-and-swap primitive.
    #[test]
    fn t11_state_compare_and_swap() {
        let tmp = fixture_project();
        let dir = write_loop_config(&tmp.path().join(".ac"), &sample_config()).expect("config");
        let state_path = dir.join(LOOP_STATE_FILE);

        let (state, raw) = read_loop_state_with_raw(&dir).expect("absent");
        assert!(raw.is_none());
        assert_eq!(
            serde_json::to_string(&state).expect("json"),
            serde_json::to_string(&LoopState::default()).expect("json")
        );

        let first = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };
        assert_eq!(
            write_loop_state_if_unchanged(&dir, &first, None).expect("absent/absent"),
            LoopStateWrite::Written
        );
        let (_, raw) = read_loop_state_with_raw(&dir).expect("present");
        let raw = raw.expect("raw present");
        assert_eq!(raw, std::fs::read_to_string(&state_path).expect("bytes"));

        assert_eq!(
            write_loop_state_if_unchanged(&dir, &LoopState::default(), Some(&raw)).expect("match"),
            LoopStateWrite::Written
        );

        let before = std::fs::read(&state_path).expect("before");
        assert_eq!(
            write_loop_state_if_unchanged(&dir, &first, Some(&raw)).expect("changed underneath"),
            LoopStateWrite::Stale
        );
        assert_eq!(
            write_loop_state_if_unchanged(&dir, &first, None).expect("expected absent"),
            LoopStateWrite::Stale
        );
        assert_eq!(std::fs::read(&state_path).expect("after"), before);
        assert!(tmp_files_in(&dir).is_empty());
    }
}
