use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

use crate::config::ac_root::existing_ac_root;
use crate::config::local_config_io::{acquire_sidecar_write_lock, SidecarWriteLock};
use crate::config::naming_migration;

pub const LOOP_DIR_PREFIX: &str = "_loop_";
pub const LOOP_CONFIG_FILE: &str = "config.toml";
/// #2718 (B4b) - the Loop state name, composed once in the registry.
pub const LOOP_STATE_FILE: &str = crate::config::instance_artifacts::LOOP_STATE_TARGET_NAME;
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
    TargetUnresolved,
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
    write_config_via_tmp(&tmp_path, &config_path, &content)?;
    Ok(dir)
}

/// Writes `content` to `tmp_path` and replaces `config_path` with it. On any
/// failure, including a partial tmp write, the tmp file is removed.
fn write_config_via_tmp(tmp_path: &Path, config_path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(tmp_path, content)
        .and_then(|()| replace_file_with_retry(tmp_path, config_path))
        .map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            format!("Failed to write Loop config: {}", e)
        })
}

pub fn read_loop_state(loop_dir: &Path) -> Result<LoopState, String> {
    read_loop_state_with_raw(loop_dir).map(|(state, _)| state)
}

/// Reads `state.json` and also returns its raw text (`None` when absent), the
/// expectation `write_loop_state_if_unchanged` compares against.
pub fn read_loop_state_with_raw(loop_dir: &Path) -> Result<(LoopState, Option<String>), String> {
    let state_path = resolved_loop_state_path(loop_dir);
    if !state_path.exists() {
        return Ok((LoopState::default(), None));
    }
    let content = std::fs::read_to_string(&state_path)
        .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
    let state = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))?;
    Ok((state, Some(content)))
}

/// #2718 (B4b) - the pre-migration state name, moved to `LOOP_STATE_FILE`.
const LEGACY_LOOP_STATE_RENAME: naming_migration::Rename = naming_migration::Rename {
    from: "state.json",
    to: LOOP_STATE_FILE,
};

/// #2718 (B4b) - the state file an unlocked reader should read: the new name
/// when it exists, else the old name when that exists, else the new name. A
/// read fallback for a Loop whose lock has not been taken since the upgrade;
/// writers never use it, and the compare-and-swap does not either.
pub(crate) fn resolved_loop_state_path(loop_dir: &Path) -> PathBuf {
    let new = loop_dir.join(LOOP_STATE_FILE);
    if new.exists() {
        return new;
    }
    let old = loop_dir.join(LEGACY_LOOP_STATE_RENAME.from);
    if old.exists() {
        old
    } else {
        new
    }
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
    // #2718 4.5 point 5: resolved once; the compare and the write share it.
    let state_path = resolved_loop_state_path(loop_dir);
    let current_raw = match std::fs::read_to_string(&state_path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Failed to read {}: {}", state_path.display(), e)),
    };
    if current_raw.as_deref() != expected_raw {
        return Ok(LoopStateWrite::Stale);
    }
    loop_write_pause_hook("after_cas_compare", loop_dir);
    write_loop_state_atomic_at(loop_dir, &state_path, state)?;
    Ok(LoopStateWrite::Written)
}

/// #2718 4.5 point 4: resolves, so an unmigrated Loop keeps writing its old
/// name until a sweep and a rename have both succeeded.
pub fn write_loop_state_atomic(loop_dir: &Path, state: &LoopState) -> Result<(), String> {
    write_loop_state_atomic_at(loop_dir, &resolved_loop_state_path(loop_dir), state)
}

/// The atomic write onto an already resolved `state_path`; the temporary is
/// named after that file.
fn write_loop_state_atomic_at(
    loop_dir: &Path,
    state_path: &Path,
    state: &LoopState,
) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let state_name = state_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(LOOP_STATE_FILE);
    let tmp_path = loop_dir.join(format!("{}.{}.tmp", state_name, Uuid::new_v4()));
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize Loop state: {}", e))?;
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temporary Loop state: {}", e))?;
    replace_file_with_retry(&tmp_path, state_path).map_err(|e| {
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
    if config.loop_def.enabled {
        resolve_loop_target(project_dir, config)
            .map_err(|e| format!("{e}. Disable the Loop or choose an existing room."))?;
    }
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

/// #2682 - production deadline for [`acquire_loop_lock`].
pub const LOOP_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// #2682 - a held Loop lock; dropping it releases the OS lock.
pub(crate) type LoopDirLock = SidecarWriteLock;

/// #2682 - cross-process guard over one Loop's `config.toml` + `state.json` +
/// `audit.jsonl`, and over the existence of the Loop directory itself.
///
/// The sidecar is `<canonical ac_root>/._loop_<id>.lock`: outside the Loop
/// directory, so it resolves before `create` makes the directory and survives
/// `remove_dir_all`. Rule: in every section that reads, checks or mutates a
/// Loop, this is the first statement once the AC root and Loop id are known;
/// nothing that observes or mutates the Loop runs above it. The leaf writers
/// (`write_loop_config`, `write_loop_state_atomic`, `append_loop_audit_once`)
/// never take it: the OS lock excludes per handle, so a self-locking leaf
/// would deadlock against its own section. Lock order: `scan_lock` ->
/// `io_lock` -> this. Never held across an `.await`.
pub(crate) fn acquire_loop_lock(
    ac_root: &Path,
    loop_id: &str,
    timeout: Duration,
) -> Result<LoopDirLock, String> {
    acquire_loop_lock_in(
        ac_root,
        loop_id,
        timeout,
        crate::config::config_dir().as_deref(),
    )
}

/// #2718 (B4b) - [`acquire_loop_lock`] with an explicit journal directory. The
/// Loop state migration runs here, under the lock just taken, before the guard
/// is returned, so every Loop section passes through it.
fn acquire_loop_lock_in(
    ac_root: &Path,
    loop_id: &str,
    timeout: Duration,
    journal_dir: Option<&Path>,
) -> Result<LoopDirLock, String> {
    validate_loop_id(loop_id)?;
    let canonical_root = std::fs::canonicalize(ac_root).map_err(|e| {
        format!(
            "Failed to resolve AC root '{}' for Loop lock: {}",
            ac_root.display(),
            e
        )
    })?;
    let lock_path = canonical_root.join(format!(".{}{}.lock", LOOP_DIR_PREFIX, loop_id));
    let guard =
        acquire_sidecar_write_lock(&lock_path, timeout, "loopLockTimeout", "Loop write lock")?;
    migrate_loop_state_name(&canonical_root, loop_id, journal_dir);
    Ok(guard)
}

/// #2682 - [`acquire_loop_lock`] for a caller that holds only the Loop
/// directory path (`<ac_root>/_loop_<id>`), such as the scheduler.
pub(crate) fn acquire_loop_lock_for_dir(
    dir: &Path,
    timeout: Duration,
) -> Result<LoopDirLock, String> {
    acquire_loop_lock_for_dir_in(dir, timeout, crate::config::config_dir().as_deref())
}

/// #2718 (B4b) - [`acquire_loop_lock_for_dir`] with an explicit journal directory.
fn acquire_loop_lock_for_dir_in(
    dir: &Path,
    timeout: Duration,
    journal_dir: Option<&Path>,
) -> Result<LoopDirLock, String> {
    let ac_root = dir
        .parent()
        .ok_or_else(|| format!("Loop directory {} has no parent", dir.display()))?;
    let loop_id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(LOOP_DIR_PREFIX))
        .ok_or_else(|| format!("{} is not a Loop directory", dir.display()))?;
    acquire_loop_lock_in(ac_root, loop_id, timeout, journal_dir)
}

/// #2718 (B4b) - one naming-migration scope per canonical Loop directory.
fn loop_state_scope_key(loop_dir: &Path) -> String {
    format!("loop-state:{}", loop_dir.display())
}

/// #2718 (B4b) - moves `_loop_<id>/state.json` to `LOOP_STATE_FILE`, journalled
/// in `journal_dir`. It runs under the Loop lock the caller already holds and
/// takes **no** lock of its own: `lock_scope` would open that sidecar on a
/// second handle and deadlock against the caller (see [`acquire_loop_lock`]),
/// so `lock_scope_under_held(.., None, ..)` is used only for the proof token.
/// Fail-soft: a refusal leaves the Loop on its old name, which
/// `resolved_loop_state_path` still reads, and the next lock retries.
fn migrate_loop_state_name(canonical_root: &Path, loop_id: &str, journal_dir: Option<&Path>) {
    let loop_dir = canonical_root.join(format!("{LOOP_DIR_PREFIX}{loop_id}"));
    if !loop_dir.is_dir() {
        return;
    }
    let key = loop_state_scope_key(&loop_dir);
    let warn = |what: &str, refusal: naming_migration::Refusal| {
        let reason = match refusal {
            naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
            naming_migration::Refusal::Io(message) => message,
        };
        log::warn!(
            "[loops] #2718 - {what} for {}: {reason}; the Loop keeps its old state name until the \
                next lock",
            loop_dir.display()
        );
    };
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    match naming_migration::read_journal(journal_dir) {
        Ok(journal) if naming_migration::scope_is_settled(journal.as_ref(), &key) => return,
        Ok(_) => {}
        Err(refusal) => return warn("the naming-migration journal is unreadable", refusal),
    }
    let held = match naming_migration::lock_scope_under_held(
        &loop_dir,
        None,
        naming_migration::MIGRATION_LOCK_BUDGET,
    ) {
        Ok(held) => held,
        Err(refusal) => return warn("the scope token failed", refusal),
    };
    // #2718 4.5: the new names are ignored before anything takes them.
    if let Err(refusal) = ensure_loop_state_ignore_rows(canonical_root) {
        return warn("the .ac/.gitignore sweep failed", refusal);
    }
    if let naming_migration::Outcome::Refused(refusal) = naming_migration::rename_step(
        &loop_dir,
        &LEGACY_LOOP_STATE_RENAME,
        &key,
        journal_dir,
        &held,
    ) {
        return warn("the state rename was refused", refusal);
    }
    if let Err(refusal) = naming_migration::update_journal(journal_dir, |j| {
        j.set_status(&key, naming_migration::ScopeStatus::Complete)
    }) {
        warn("the scope completion was not recorded", refusal);
    }
}

#[cfg(not(test))]
#[inline(always)]
fn sweep_pause_hook(_stage: &str, _dir: &Path) {}

/// Test-only pause point between the sweep's read and its append. Inert unless
/// a test arms it.
#[cfg(test)]
fn sweep_pause_hook(stage: &str, dir: &Path) {
    naming_migration::pause::hook(stage, dir);
}

/// #2718 (B4b) 4.5 - appends to `<ac_root>/.gitignore` the Loop state rows it
/// lacks. An appending write, never a rewrite, so no line another writer added
/// can be lost; retirement of the old row stays with the writer at registration.
fn ensure_loop_state_ignore_rows(ac_root: &Path) -> Result<(), naming_migration::Refusal> {
    let path = ac_root.join(".gitignore");
    let io = |what: &str, e: std::io::Error| {
        naming_migration::Refusal::Io(format!("failed to {what} {}: {e}", path.display()))
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io("read", e)),
    };
    let blocks = naming_migration::missing_ignore_rows(
        &content,
        &naming_migration::loop_state_ignore_rows(),
    );
    if blocks.is_empty() {
        return Ok(());
    }
    sweep_pause_hook("before_gitignore_append", ac_root);
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(format!("{separator}{blocks}").as_bytes()))
        .map_err(|e| io("append to", e))
}

/// #2682 - the create section shared by the app command and the CLI: lock,
/// existence check, validation, then the config+state pair as one unit.
pub fn create_loop_files(
    project_dir: &Path,
    ac_root: &Path,
    config: &LoopConfigToml,
) -> Result<(PathBuf, LoopState), String> {
    let _lock = acquire_loop_lock(ac_root, &config.loop_def.id, LOOP_LOCK_TIMEOUT)?;
    if loop_dir(ac_root, &config.loop_def.id).exists() {
        return Err(format!("Loop '{}' already exists", config.loop_def.id));
    }
    validate_loop_config(project_dir, config)?;
    let dir = write_loop_config(ac_root, config)?;
    loop_write_pause_hook("between_config_and_state", &dir);
    let state = baseline_loop_state(config, Utc::now())?;
    write_loop_state_atomic(&dir, &state)?;
    Ok((dir, state))
}

/// #2682 - the read-modify-write section shared by the app `update_loop` /
/// `toggle_loop` and the CLI `update` / `enable` / `disable`.
pub fn update_loop_files(
    project_dir: &Path,
    ac_root: &Path,
    loop_id: &str,
    patch: LoopUpdatePatch,
) -> Result<(PathBuf, LoopConfigToml, LoopState), String> {
    let _lock = acquire_loop_lock(ac_root, loop_id, LOOP_LOCK_TIMEOUT)?;
    let dir = loop_dir(ac_root, loop_id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", loop_id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(&mut config, patch)?;
    validate_loop_config(project_dir, &config)?;
    let dir = write_loop_config(ac_root, &config)?;
    loop_write_pause_hook("between_config_and_state", &dir);
    let state = if reset_schedule {
        baseline_loop_state(&config, Utc::now())?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
    Ok((dir, config, state))
}

/// #2682 - the delete section shared by the app `delete_loop` and the CLI
/// `remove`. The sidecar lives outside the directory it deletes.
pub fn remove_loop_files(ac_root: &Path, loop_id: &str) -> Result<PathBuf, String> {
    let _lock = acquire_loop_lock(ac_root, loop_id, LOOP_LOCK_TIMEOUT)?;
    let dir = loop_dir(ac_root, loop_id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", loop_id));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Failed to remove Loop directory: {}", e))?;
    Ok(dir)
}

#[cfg(not(test))]
#[inline(always)]
fn loop_write_pause_hook(_stage: &str, _dir: &Path) {}

/// #2682 - test-only pause point. Inert unless a test arms it.
#[cfg(test)]
fn loop_write_pause_hook(stage: &str, dir: &Path) {
    pause::hook(stage, dir);
}

#[cfg(test)]
pub(crate) mod pause;

#[cfg(test)]
mod tests;
