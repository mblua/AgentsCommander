use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::config::loops::{
    append_loop_audit_once, baseline_loop_state, details_from_parts, latest_due_between, loop_dir,
    next_due_after, read_loop_config, read_loop_state, write_loop_state_atomic, LoopAuditEntry,
    LoopAuditKind, LoopConfigDetails, LoopConfigToml, LoopLastResult, LoopState, LOOP_CONFIG_FILE,
    LOOP_DIR_PREFIX,
};
use crate::config::projects::enumerate_registered_project_candidates;
use crate::config::settings::SettingsState;
use crate::config::workspace::existing_workspace_dir;
use crate::loops::delivery::{deliver_loop_prompt, LoopDeliveryReport};
use crate::shutdown::ShutdownSignal;

pub struct LoopScheduler {
    notify: tokio::sync::Notify,
    scan_lock: tokio::sync::Mutex<()>,
}

impl LoopScheduler {
    pub fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            scan_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn request_scan(&self) {
        self.notify.notify_one();
    }

    pub async fn mutation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.scan_lock.lock().await
    }

    pub fn start(self: Arc<Self>, app: AppHandle, shutdown: ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            self.scan_once(app.clone(), true, false).await;
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = shutdown.token().cancelled() => break,
                    _ = interval.tick() => self.scan_once(app.clone(), false, false).await,
                    _ = self.notify.notified() => self.scan_once(app.clone(), false, false).await,
                }
            }
        });
    }

    pub async fn on_session_idle(&self, app: AppHandle, _session_id: Uuid) {
        self.scan_once(app, false, true).await;
    }

    pub async fn run_loop_now(
        &self,
        app: AppHandle,
        project_dir: PathBuf,
        loop_id: String,
    ) -> Result<LoopConfigDetails, String> {
        let _guard = self.scan_lock.lock().await;
        let workspace_dir = existing_workspace_dir(&project_dir).ok_or_else(|| {
            format!(
                "Project AC Root not found in {} (.ac)",
                project_dir.display()
            )
        })?;
        let dir = loop_dir(&workspace_dir, &loop_id);
        if !dir.is_dir() {
            return Err(format!("Loop '{}' not found", loop_id));
        }
        let config = read_loop_config(&dir)?;
        if !config.loop_def.enabled {
            return Err(format!("Loop '{}' is disabled", loop_id));
        }
        let state = read_loop_state(&dir).unwrap_or_default();
        let run_id = Uuid::new_v4();
        let due_at = Utc::now();
        let started_at = Utc::now();
        if !loop_is_current_for_delivery(&dir, &config)? {
            return Err(format!("Loop '{}' changed before delivery", loop_id));
        }
        let report = deliver_loop_prompt(&app, &project_dir, &config, run_id, due_at).await;
        let state = self
            .apply_delivery_report(
                &app,
                &project_dir,
                &dir,
                &config,
                state,
                report,
                run_id,
                due_at,
                started_at,
            )
            .await?;
        Ok(details_from_parts(&dir, &config, &state))
    }

    async fn scan_once(&self, app: AppHandle, startup: bool, pending_only: bool) {
        let _guard = self.scan_lock.lock().await;
        let project_paths = {
            let settings = app.state::<SettingsState>();
            let project_paths = settings.read().await.project_paths.clone();
            project_paths
        };
        let projects = enumerate_registered_project_candidates(&project_paths);
        for project in projects {
            if let Err(e) = self
                .scan_project(&app, &project.path, startup, pending_only)
                .await
            {
                log::warn!(
                    "[loops] Scheduler scan failed for project {}: {}",
                    project.path.display(),
                    e
                );
            }
        }
    }

    async fn scan_project(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        startup: bool,
        pending_only: bool,
    ) -> Result<(), String> {
        let Some(workspace_dir) = existing_workspace_dir(project_dir) else {
            return Ok(());
        };
        let loop_dirs = loop_dirs(&workspace_dir)?;
        for dir in loop_dirs {
            if let Err(e) = self
                .scan_loop(app, project_dir, &dir, startup, pending_only)
                .await
            {
                log::warn!("[loops] Loop scan failed for {}: {}", dir.display(), e);
            }
        }
        Ok(())
    }

    async fn scan_loop(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        startup: bool,
        pending_only: bool,
    ) -> Result<(), String> {
        let config = read_loop_config(dir)?;
        if !config.loop_def.enabled {
            return Ok(());
        }
        let mut state = read_loop_state(dir).unwrap_or_else(|e| {
            log::warn!(
                "[loops] Treating unreadable Loop state as default for {}: {}",
                dir.display(),
                e
            );
            LoopState::default()
        });

        if state.last_checked_at.is_none() {
            if !loop_is_current_for_delivery(dir, &config)? {
                return Ok(());
            }
            state = baseline_loop_state(&config, Utc::now())?;
            write_loop_state_atomic(dir, &state)?;
            return Ok(());
        }

        if let (Some(pending_due), Some(pending_run)) = (state.pending_due_at, state.pending_run_id)
        {
            if !loop_is_current_for_delivery(dir, &config)? {
                return Ok(());
            }
            let started_at = Utc::now();
            let report =
                deliver_loop_prompt(app, project_dir, &config, pending_run, pending_due).await;
            if report.kind == LoopAuditKind::PendingBusy {
                self.maybe_coalesce_pending(app, project_dir, dir, &config, &mut state)
                    .await?;
                return Ok(());
            }
            self.apply_delivery_report(
                app,
                project_dir,
                dir,
                &config,
                state,
                report,
                pending_run,
                pending_due,
                started_at,
            )
            .await?;
            return Ok(());
        }

        if pending_only {
            return Ok(());
        }

        let now = Utc::now();
        let last_checked = state.last_checked_at.unwrap_or(now);
        let Some(due_at) = latest_due_between(&config.trigger.expr, last_checked, now)? else {
            return Ok(());
        };

        let run_id = Uuid::new_v4();
        if startup {
            if !loop_is_current_for_delivery(dir, &config)? {
                return Ok(());
            }
            self.record_missed_while_closed(app, project_dir, dir, &config, state, run_id, due_at)
                .await?;
            return Ok(());
        }

        let started_at = Utc::now();
        if !loop_is_current_for_delivery(dir, &config)? {
            return Ok(());
        }
        let report = deliver_loop_prompt(app, project_dir, &config, run_id, due_at).await;
        self.apply_delivery_report(
            app,
            project_dir,
            dir,
            &config,
            state,
            report,
            run_id,
            due_at,
            started_at,
        )
        .await?;
        Ok(())
    }

    async fn maybe_coalesce_pending(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        config: &LoopConfigToml,
        state: &mut LoopState,
    ) -> Result<(), String> {
        if !loop_is_current_for_delivery(dir, config)? {
            return Ok(());
        }
        let Some(pending_due) = state.pending_due_at else {
            return Ok(());
        };
        let now = Utc::now();
        let Some(new_due) = latest_due_between(&config.trigger.expr, pending_due, now)? else {
            return Ok(());
        };
        if new_due <= pending_due {
            return Ok(());
        }
        let run_id = state.pending_run_id.unwrap_or_else(Uuid::new_v4);
        state.last_checked_at = Some(now);
        state.last_due_at = Some(new_due);
        state.next_due_at = next_due_after(&config.trigger.expr, now)?;
        state.last_result = Some(LoopLastResult {
            kind: "coalescedPending".to_string(),
            message: "A new due time occurred while the prior run was still pending".to_string(),
        });
        append_loop_audit_once(
            dir,
            &LoopAuditEntry {
                run_id,
                loop_id: config.loop_def.id.clone(),
                project_path: project_dir.to_string_lossy().to_string(),
                kind: LoopAuditKind::CoalescedPending,
                due_at: new_due,
                started_at: now,
                completed_at: Some(now),
                target: None,
                session_id: None,
                busy_coordinator_policy: config.policy.busy_coordinator.clone(),
                error: None,
                prompt_snapshot: None,
            },
        )?;
        write_loop_state_atomic(dir, state)?;
        emit_transition(
            app,
            project_dir,
            dir,
            config,
            state,
            "coalesced",
            Some("A due run was coalesced into the pending Loop delivery".to_string()),
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_missed_while_closed(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        config: &LoopConfigToml,
        mut state: LoopState,
        run_id: Uuid,
        due_at: DateTime<Utc>,
    ) -> Result<LoopState, String> {
        if !loop_is_current_for_delivery(dir, config)? {
            return Ok(state);
        }
        let now = Utc::now();
        state.last_checked_at = Some(now);
        state.last_due_at = Some(due_at);
        state.last_missed_closed_at = Some(due_at);
        state.next_due_at = next_due_after(&config.trigger.expr, now)?;
        state.last_result = Some(LoopLastResult {
            kind: "missedWhileClosed".to_string(),
            message: "Loop was due while AgentsCommander was closed; no catch-up run was injected"
                .to_string(),
        });
        append_loop_audit_once(
            dir,
            &LoopAuditEntry {
                run_id,
                loop_id: config.loop_def.id.clone(),
                project_path: project_dir.to_string_lossy().to_string(),
                kind: LoopAuditKind::MissedWhileClosed,
                due_at,
                started_at: now,
                completed_at: Some(now),
                target: None,
                session_id: None,
                busy_coordinator_policy: config.policy.busy_coordinator.clone(),
                error: None,
                prompt_snapshot: None,
            },
        )?;
        write_loop_state_atomic(dir, &state)?;
        emit_transition(
            app,
            project_dir,
            dir,
            config,
            &state,
            "missed",
            Some("Loop was missed while AgentsCommander was closed".to_string()),
        );
        Ok(state)
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_delivery_report(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        config: &LoopConfigToml,
        mut state: LoopState,
        report: LoopDeliveryReport,
        run_id: Uuid,
        due_at: DateTime<Utc>,
        started_at: DateTime<Utc>,
    ) -> Result<LoopState, String> {
        if !loop_is_current_for_delivery(dir, config)? {
            return Ok(state);
        }
        let now = Utc::now();
        state.last_checked_at = Some(now);
        state.last_due_at = Some(due_at);
        state.next_due_at = next_due_after(&config.trigger.expr, now)?;
        state.last_result = Some(LoopLastResult {
            kind: audit_kind_name(&report.kind).to_string(),
            message: report.message.clone(),
        });

        match report.kind {
            LoopAuditKind::Delivered => {
                state.last_delivered_at = report.completed_at.or(Some(now));
                state.pending_due_at = None;
                state.pending_run_id = None;
            }
            LoopAuditKind::PendingBusy => {
                state.pending_due_at = Some(due_at);
                state.pending_run_id = Some(run_id);
            }
            LoopAuditKind::SkippedBusy | LoopAuditKind::DeliveryFailed => {
                state.pending_due_at = None;
                state.pending_run_id = None;
            }
            LoopAuditKind::MissedWhileClosed | LoopAuditKind::CoalescedPending => {}
        }

        append_loop_audit_once(
            dir,
            &LoopAuditEntry {
                run_id,
                loop_id: config.loop_def.id.clone(),
                project_path: project_dir.to_string_lossy().to_string(),
                kind: report.kind.clone(),
                due_at,
                started_at,
                completed_at: report.completed_at,
                target: report.target.clone(),
                session_id: report.session_id,
                busy_coordinator_policy: config.policy.busy_coordinator.clone(),
                error: report.error.clone(),
                prompt_snapshot: report.prompt_snapshot.clone(),
            },
        )?;
        write_loop_state_atomic(dir, &state)?;
        emit_transition(
            app,
            project_dir,
            dir,
            config,
            &state,
            audit_kind_event(&report.kind),
            Some(report.message),
        );
        Ok(state)
    }
}

impl Default for LoopScheduler {
    fn default() -> Self {
        Self::new()
    }
}

fn loop_dirs(workspace_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(workspace_dir)
        .map_err(|e| format!("Failed to read Project AC Root: {}", e))?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopRevalidation {
    Current,
    Gone,
    Disabled,
    Changed,
}

fn loop_is_current_for_delivery(dir: &Path, expected: &LoopConfigToml) -> Result<bool, String> {
    match revalidate_loop_current(dir, expected)? {
        LoopRevalidation::Current => Ok(true),
        LoopRevalidation::Gone => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because {} no longer exists",
                dir.display()
            );
            Ok(false)
        }
        LoopRevalidation::Disabled => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because '{}' is disabled",
                expected.loop_def.id
            );
            Ok(false)
        }
        LoopRevalidation::Changed => {
            log::warn!(
                "[loops] Skipping stale Loop delivery because '{}' changed",
                expected.loop_def.id
            );
            Ok(false)
        }
    }
}

fn revalidate_loop_current(
    dir: &Path,
    expected: &LoopConfigToml,
) -> Result<LoopRevalidation, String> {
    if !dir.is_dir() || !dir.join(LOOP_CONFIG_FILE).is_file() {
        return Ok(LoopRevalidation::Gone);
    }
    let current = read_loop_config(dir)?;
    if !current.loop_def.enabled {
        return Ok(LoopRevalidation::Disabled);
    }
    if loop_delivery_config_matches(&current, expected) {
        Ok(LoopRevalidation::Current)
    } else {
        Ok(LoopRevalidation::Changed)
    }
}

fn loop_delivery_config_matches(current: &LoopConfigToml, expected: &LoopConfigToml) -> bool {
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

fn emit_transition(
    app: &AppHandle,
    project_dir: &Path,
    dir: &Path,
    config: &LoopConfigToml,
    state: &LoopState,
    kind: &str,
    message: Option<String>,
) {
    let details = details_from_parts(dir, config, state);
    crate::commands::loops::emit_loop_change(
        app,
        project_dir,
        dir,
        &config.loop_def.id,
        kind,
        Some(details.summary),
        message,
    );
}

fn audit_kind_name(kind: &LoopAuditKind) -> &'static str {
    match kind {
        LoopAuditKind::Delivered => "delivered",
        LoopAuditKind::PendingBusy => "pendingBusy",
        LoopAuditKind::SkippedBusy => "skippedBusy",
        LoopAuditKind::MissedWhileClosed => "missedWhileClosed",
        LoopAuditKind::DeliveryFailed => "deliveryFailed",
        LoopAuditKind::CoalescedPending => "coalescedPending",
    }
}

fn audit_kind_event(kind: &LoopAuditKind) -> &'static str {
    match kind {
        LoopAuditKind::Delivered => "delivered",
        LoopAuditKind::PendingBusy => "pending",
        LoopAuditKind::SkippedBusy => "skipped",
        LoopAuditKind::MissedWhileClosed => "missed",
        LoopAuditKind::DeliveryFailed => "failed",
        LoopAuditKind::CoalescedPending => "coalesced",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loops::{
        write_loop_config, BusyCoordinatorPolicy, LoopDef, LoopPolicy, LoopPrompt, LoopTarget,
        LoopTargetKind, LoopTrigger, LoopTriggerKind, LOOP_TIMEZONE_LOCAL,
    };

    fn sample_config() -> LoopConfigToml {
        LoopConfigToml {
            loop_def: LoopDef {
                id: "daily-sync".to_string(),
                name: "Daily sync".to_string(),
                enabled: true,
            },
            trigger: LoopTrigger {
                kind: LoopTriggerKind::Cron,
                expr: "0 9 * * *".to_string(),
                timezone: LOOP_TIMEZONE_LOCAL.to_string(),
            },
            target: LoopTarget {
                kind: LoopTargetKind::WorkgroupCoordinator,
                workgroup: "wg-1-dev-team".to_string(),
            },
            prompt: LoopPrompt {
                body: "Send status".to_string(),
            },
            policy: LoopPolicy {
                busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
                ..LoopPolicy::default()
            },
        }
    }

    #[test]
    fn revalidate_loop_current_detects_deleted_disabled_and_changed_configs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = sample_config();
        let dir = write_loop_config(tmp.path(), &config).expect("write config");

        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("current"),
            LoopRevalidation::Current
        );

        let mut renamed = config.clone();
        renamed.loop_def.name = "Renamed sync".to_string();
        write_loop_config(tmp.path(), &renamed).expect("write renamed config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("renamed"),
            LoopRevalidation::Current
        );

        let mut retargeted = config.clone();
        retargeted.target.workgroup = "wg-2-dev-team".to_string();
        write_loop_config(tmp.path(), &retargeted).expect("write retargeted config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("retargeted"),
            LoopRevalidation::Changed
        );

        let mut disabled = config.clone();
        disabled.loop_def.enabled = false;
        write_loop_config(tmp.path(), &disabled).expect("write disabled config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("disabled"),
            LoopRevalidation::Disabled
        );

        std::fs::remove_dir_all(&dir).expect("remove loop dir");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("gone"),
            LoopRevalidation::Gone
        );
    }
}
