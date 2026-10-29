use std::path::{Path, PathBuf};

use super::{SchedulerContext, UnresolvedLoopTarget};
use crate::config::ac_root::existing_ac_root;
use crate::config::loops::{
    details_from_parts, read_loop_config, read_loop_config_if_present, read_loop_state_with_raw,
    resolve_loop_target, revalidate_loop_current, write_loop_state_atomic,
    write_loop_state_if_unchanged, LoopAuditKind, LoopConfigRevalidation, LoopConfigToml,
    LoopState, LoopStateWrite, LOOP_DIR_PREFIX,
};
use crate::config::projects::{enumerate_registered_project_candidates, ProjectResolution};
use crate::config::sessions_persistence;
use crate::config::settings::SettingsState;
use crate::filesystem::loop_files;
use crate::loops::events::emit_loop_change;

/// Every scan-side state write: a Loop whose `config.toml` is gone is `Stale`
/// with nothing written, else a compare-and-swap against `s0_raw`.
pub(super) fn guarded_state_write(
    dir: &Path,
    state: &LoopState,
    s0_raw: Option<&[u8]>,
) -> Result<LoopStateWrite, String> {
    if read_loop_config_if_present(dir)?.is_none() {
        return Ok(LoopStateWrite::Stale);
    }
    match s0_raw.map(std::str::from_utf8) {
        None => write_loop_state_if_unchanged(dir, state, None),
        Some(Ok(text)) => write_loop_state_if_unchanged(dir, state, Some(text)),
        // Not UTF-8, so phase A's text CAS cannot read it: compare the bytes
        // here, under `io_lock`, then replace the unreadable state.
        Some(Err(_)) => {
            if read_raw_loop_state(dir)?.as_deref() != s0_raw {
                return Ok(LoopStateWrite::Stale);
            }
            write_loop_state_atomic(dir, state)?;
            Ok(LoopStateWrite::Written)
        }
    }
}

/// Canonical parent plus directory name, so spelling aliases share one key
/// and the key survives the directory's own deletion. Case-folded on Windows.
pub(super) fn generation_key(dir: &Path) -> String {
    let path = match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) => loop_files::canonical_loop_root(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| dir.to_path_buf()),
        _ => dir.to_path_buf(),
    };
    let key = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// The scan's view of `state.json`: parsed state plus raw bytes. An
/// unreadable state (unparsable or not UTF-8) is treated as default, as
/// before, but keeps its bytes so the compare-and-swap can still replace it.
pub(super) fn read_state_snapshot(dir: &Path) -> Result<(LoopState, Option<Vec<u8>>), String> {
    match read_loop_state_with_raw(dir) {
        Ok((state, raw)) => Ok((state, raw.map(String::into_bytes))),
        Err(e) => {
            log::warn!(
                "[loops] Treating unreadable Loop state as default for {}: {}",
                dir.display(),
                e
            );
            Ok((LoopState::default(), read_raw_loop_state(dir)?))
        }
    }
}

pub(super) fn read_raw_loop_state(dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = crate::config::loops::resolved_loop_state_path(dir);
    match loop_files::read_loop_bytes(&path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read {}: {}", path.display(), e)),
    }
}

pub(super) async fn active_project_candidates_for_owner(
    owner: &SettingsState,
) -> Vec<ProjectResolution> {
    let (project_paths, archived) = {
        let s = match owner.current_read().await {
            Ok(settings) => settings,
            Err(error) => {
                log::debug!("[loops] candidate eligibility unavailable: {error}");
                return Vec::new();
            }
        };
        (s.project_paths.clone(), s.archived_project_paths.clone())
    };
    let projects = enumerate_registered_project_candidates(&project_paths);
    let archived_roots = sessions_persistence::normalize_project_roots(&archived);
    retain_unarchived_candidates(projects, &archived_roots)
}

pub(super) fn unresolved_targets_in_project(project_dir: &Path) -> Vec<UnresolvedLoopTarget> {
    let mut alerts = Vec::new();
    let Some(ac_root) = existing_ac_root(project_dir) else {
        return alerts;
    };
    let dirs = match loop_dirs(&ac_root) {
        Ok(dirs) => dirs,
        Err(e) => {
            log::warn!(
                "[loops] Failed to list Loops for project {}: {}",
                project_dir.display(),
                e
            );
            return alerts;
        }
    };
    for dir in dirs {
        let config = match read_loop_config(&dir) {
            Ok(config) => config,
            Err(e) => {
                log::warn!(
                    "[loops] Failed to read Loop config {}: {}",
                    dir.display(),
                    e
                );
                continue;
            }
        };
        if !config.loop_def.enabled {
            continue;
        }
        if let Err(e) = resolve_loop_target(project_dir, &config) {
            alerts.push(UnresolvedLoopTarget {
                project_path: project_dir.to_string_lossy().into(),
                loop_id: config.loop_def.id.clone(),
                loop_name: config.loop_def.name.clone(),
                workgroup: config.target.workgroup.clone(),
                error: e,
            });
        }
    }
    alerts
}

pub(super) fn loop_dirs(ac_root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = loop_files::loop_discovery_entries(ac_root)
        .map_err(|e| format!("Failed to read Project AC Root: {}", e))?;
    let mut dirs = Vec::new();
    for path in entries {
        if !loop_files::loop_path_is_directory(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(LOOP_DIR_PREFIX) {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

pub(super) fn loop_is_current_for_delivery(
    dir: &Path,
    expected: &LoopConfigToml,
) -> Result<bool, String> {
    match revalidate_loop_current(dir, expected)? {
        LoopConfigRevalidation::Current => Ok(true),
        LoopConfigRevalidation::Gone => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because {} no longer exists",
                dir.display()
            );
            Ok(false)
        }
        LoopConfigRevalidation::Disabled => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because '{}' is disabled",
                expected.loop_def.id
            );
            Ok(false)
        }
        LoopConfigRevalidation::Changed => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because '{}' changed",
                expected.loop_def.id
            );
            Ok(false)
        }
    }
}

/// #881 A5: candidate discovery also yields immediate `.ac`-bearing children
/// of registered paths. Subtract archived roots so nested archived projects do
/// not keep firing loops from a registered parent.
pub(super) fn retain_unarchived_candidates(
    candidates: Vec<ProjectResolution>,
    normalized_archived_roots: &[String],
) -> Vec<ProjectResolution> {
    if normalized_archived_roots.is_empty() {
        return candidates;
    }
    candidates
        .into_iter()
        .filter(|candidate| {
            !sessions_persistence::is_under_normalized_archived_roots(
                &candidate.path.to_string_lossy(),
                normalized_archived_roots,
            )
        })
        .collect()
}

pub(super) fn emit_transition(
    context: &SchedulerContext,
    project_dir: &Path,
    dir: &Path,
    config: &LoopConfigToml,
    state: &LoopState,
    kind: &str,
    message: Option<String>,
) {
    let details = details_from_parts(dir, config, state);
    match context {
        SchedulerContext::Native(app) => emit_loop_change(
            app.as_ref(),
            project_dir,
            dir,
            &config.loop_def.id,
            kind,
            Some(details.summary),
            message,
        ),
        #[cfg(test)]
        SchedulerContext::Headless { transitions, .. } => {
            transitions.lock().expect("scheduler transitions").push((
                project_dir.to_path_buf(),
                dir.to_path_buf(),
                config.loop_def.id.clone(),
                kind.to_owned(),
                Some(details.summary),
                message,
            ));
        }
    }
}

pub(super) fn audit_kind_name(kind: &LoopAuditKind) -> &'static str {
    match kind {
        LoopAuditKind::Delivered => "delivered",
        LoopAuditKind::PendingBusy => "pendingBusy",
        LoopAuditKind::SkippedBusy => "skippedBusy",
        LoopAuditKind::MissedWhileClosed => "missedWhileClosed",
        LoopAuditKind::DeliveryFailed => "deliveryFailed",
        LoopAuditKind::CoalescedPending => "coalescedPending",
        LoopAuditKind::TargetUnresolved => "targetUnresolved",
    }
}

pub(super) fn audit_kind_event(kind: &LoopAuditKind) -> &'static str {
    match kind {
        LoopAuditKind::Delivered => "delivered",
        LoopAuditKind::PendingBusy => "pending",
        LoopAuditKind::SkippedBusy => "skipped",
        LoopAuditKind::MissedWhileClosed => "missed",
        LoopAuditKind::DeliveryFailed => "failed",
        LoopAuditKind::CoalescedPending => "coalesced",
        LoopAuditKind::TargetUnresolved => "unresolved",
    }
}
