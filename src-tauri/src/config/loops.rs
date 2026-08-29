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
}

impl Default for LoopPolicy {
    fn default() -> Self {
        Self {
            missed_while_closed: MissedWhileClosedPolicy::Notify,
            busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
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
    toml::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))
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
}

pub fn write_loop_config(ac_root: &Path, config: &LoopConfigToml) -> Result<PathBuf, String> {
    validate_loop_id(&config.loop_def.id)?;
    let dir = loop_dir(ac_root, &config.loop_def.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create Loop directory: {}", e))?;
    let content = toml::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize Loop config: {}", e))?;
    std::fs::write(dir.join(LOOP_CONFIG_FILE), content)
        .map_err(|e| format!("Failed to write Loop config: {}", e))?;
    Ok(dir)
}

pub fn read_loop_state(loop_dir: &Path) -> Result<LoopState, String> {
    let state_path = loop_dir.join(LOOP_STATE_FILE);
    if !state_path.exists() {
        return Ok(LoopState::default());
    }
    let content = std::fs::read_to_string(&state_path)
        .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))
}

pub fn write_loop_state_atomic(loop_dir: &Path, state: &LoopState) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let state_path = loop_dir.join(LOOP_STATE_FILE);
    let tmp_path = loop_dir.join(format!("{}.{}.tmp", LOOP_STATE_FILE, Uuid::new_v4()));
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize Loop state: {}", e))?;
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temporary Loop state: {}", e))?;
    replace_file(&tmp_path, &state_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize Loop state: {}", e)
    })
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
}
