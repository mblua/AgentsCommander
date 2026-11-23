mod lock;
mod model;
mod pause_hook;
mod rules;
mod storage;

use crate::config::ac_root::existing_ac_root;
#[cfg(test)]
use crate::config::naming_migration;
use crate::filesystem::loop_files;
#[cfg(test)]
use chrono::{DateTime, Utc};
pub(crate) use lock::acquire_loop_lock_for_dir;
#[cfg(test)]
pub(crate) use lock::{acquire_loop_lock, LoopDirLock};
#[cfg(test)]
use lock::{acquire_loop_lock_for_dir_in, loop_state_scope_key};
pub use lock::{create_loop_files, remove_loop_files, update_loop_files, LOOP_LOCK_TIMEOUT};
#[cfg(test)]
use pause_hook::loop_write_pause_hook;
#[cfg(test)]
pub use rules::{apply_loop_update_patch, validate_loop_config};
pub use rules::{
    baseline_loop_state, latest_due_between, next_due_after, resolve_loop_target,
    validate_cron_expr,
};
#[cfg(test)]
use std::io::Write;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::time::Duration;
#[cfg(all(test, windows))]
use storage::replace_file;
pub(crate) use storage::resolved_loop_state_path;
#[cfg(test)]
pub use storage::write_loop_config;
pub use storage::{
    append_loop_audit_once, loop_delivery_config_matches, loop_dir, read_loop_config,
    read_loop_config_if_present, read_loop_state, read_loop_state_with_raw,
    revalidate_loop_current, write_loop_state_atomic, write_loop_state_if_unchanged,
    LoopConfigRevalidation, LoopStateWrite,
};
#[cfg(test)]
use storage::{is_transient_replace_error, write_config_via_tmp};
#[cfg(test)]
use uuid::Uuid;

pub use model::{
    validate_loop_id, AcLoopSummary, BusyCoordinatorPolicy, LoopAuditEntry, LoopAuditKind,
    LoopConfigDetails, LoopConfigToml, LoopDef, LoopLastResult, LoopPolicy, LoopPrompt,
    LoopSessionStart, LoopState, LoopTarget, LoopTargetKind, LoopTrigger, LoopTriggerKind,
    LoopUpdatePatch, ResolvedLoopTarget, LOOP_CONFIG_FILE, LOOP_DIR_PREFIX, LOOP_TIMEZONE_LOCAL,
};
#[cfg(test)]
pub use model::{MissedWhileClosedPolicy, LOOP_AUDIT_FILE, LOOP_STATE_FILE};

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

pub fn discover_loops_in_project(project_dir: &Path) -> Vec<AcLoopSummary> {
    let Some(ac_root) = existing_ac_root(project_dir) else {
        return Vec::new();
    };
    let entries = match loop_files::loop_discovery_entries(&ac_root) {
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
    for dir in entries {
        if !loop_files::loop_path_is_directory(&dir) {
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

#[cfg(test)]
pub(crate) mod pause;

#[cfg(test)]
mod tests;
