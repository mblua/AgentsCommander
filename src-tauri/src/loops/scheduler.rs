use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::config::ac_root::existing_ac_root;
use crate::config::loops::{
    append_loop_audit_once, baseline_loop_state, details_from_parts, latest_due_between, loop_dir,
    next_due_after, read_loop_config, read_loop_config_if_present, read_loop_state_with_raw,
    resolve_loop_target, revalidate_loop_current, write_loop_state_atomic,
    write_loop_state_if_unchanged, LoopAuditEntry, LoopAuditKind, LoopConfigDetails,
    LoopConfigRevalidation, LoopConfigToml, LoopLastResult, LoopState, LoopStateWrite,
    LOOP_DIR_PREFIX, LOOP_STATE_FILE,
};
use crate::config::projects::{enumerate_registered_project_candidates, ProjectResolution};
use crate::config::sessions_persistence;
use crate::config::settings::SettingsState;
use crate::loops::delivery::{deliver_loop_prompt, LoopDeliveryReport};
use crate::loops::events::emit_loop_change;
use crate::shutdown::ShutdownSignal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedLoopTarget {
    pub project_path: String,
    pub loop_id: String,
    pub loop_name: String,
    pub workgroup: String,
    pub error: String,
}

/// Test-only stand-in for one delivery: signals `entered`, waits for
/// `release`, then returns `report` without calling `deliver_loop_prompt`.
#[cfg(test)]
pub(crate) struct LoopDeliveryGate {
    pub(crate) entered: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) report: Option<LoopDeliveryReport>,
}

pub struct LoopScheduler {
    notify: tokio::sync::Notify,
    /// One scan at a time. Taken by `scan_once` and `run_loop_now` and held
    /// across the delivery.
    scan_lock: tokio::sync::Mutex<()>,
    /// Short read-modify-write guard over Loop files, taken by the four Loop
    /// commands and by the scan's read and commit sections. Lock order is
    /// always `scan_lock` then `io_lock`, never the reverse. It is never held
    /// across an await other than its own acquisition, so never across a
    /// delivery. Scope: in-process writers only. `cli/loop_cmd.rs` writes
    /// Loop files from another process without this lock; the state CAS only
    /// narrows that window, it does not close it.
    io_lock: tokio::sync::Mutex<()>,
    /// Per-Loop generation, bumped by every Loop command under `io_lock`, so
    /// a scan can tell that the Loop it read was replaced underneath it.
    loop_generations: std::sync::Mutex<HashMap<String, u64>>,
    #[cfg(test)]
    delivery_gate: std::sync::Mutex<Option<LoopDeliveryGate>>,
}

impl LoopScheduler {
    pub fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            scan_lock: tokio::sync::Mutex::new(()),
            io_lock: tokio::sync::Mutex::new(()),
            loop_generations: std::sync::Mutex::new(HashMap::new()),
            #[cfg(test)]
            delivery_gate: std::sync::Mutex::new(None),
        }
    }

    pub fn request_scan(&self) {
        self.notify.notify_one();
    }

    pub async fn io_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.io_lock.lock().await
    }

    pub(crate) fn loop_generation(&self, dir: &Path) -> u64 {
        let key = generation_key(dir);
        let generations = self.loop_generations.lock().expect("loop generations");
        generations.get(&key).copied().unwrap_or(0)
    }

    pub fn bump_loop_generation(&self, dir: &Path) {
        let key = generation_key(dir);
        *self
            .loop_generations
            .lock()
            .expect("loop generations")
            .entry(key)
            .or_insert(0) += 1;
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
        let ac_root = existing_ac_root(&project_dir).ok_or_else(|| {
            format!(
                "Project AC Root not found in {} (.ac)",
                project_dir.display()
            )
        })?;
        let dir = loop_dir(&ac_root, &loop_id);
        let (config, state, s0_raw, s0_generation) = {
            let _io = self.io_lock.lock().await;
            if !dir.is_dir() {
                return Err(format!("Loop '{}' not found", loop_id));
            }
            let Some(config) = read_loop_config_if_present(&dir)? else {
                return Err(format!("Loop '{}' not found", loop_id));
            };
            let (state, raw) = read_state_snapshot(&dir)?;
            (config, state, raw, self.loop_generation(&dir))
        };
        if !config.loop_def.enabled {
            return Err(format!("Loop '{}' is disabled", loop_id));
        }
        let run_id = Uuid::new_v4();
        let due_at = Utc::now();
        let started_at = Utc::now();
        if !loop_is_current_for_delivery(&dir, &config)? {
            return Err(format!("Loop '{}' changed before delivery", loop_id));
        }
        let report = self
            .deliver(&app, &project_dir, &config, run_id, due_at)
            .await;
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
                s0_generation,
                s0_raw.as_deref(),
            )
            .await?;
        Ok(details_from_parts(&dir, &config, &state))
    }

    pub async fn unresolved_loop_targets(&self, app: &AppHandle) -> Vec<UnresolvedLoopTarget> {
        let mut alerts = Vec::new();
        for project in active_project_candidates(app).await {
            alerts.extend(unresolved_targets_in_project(&project.path));
        }
        alerts
    }

    async fn scan_once(&self, app: AppHandle, startup: bool, pending_only: bool) {
        let _guard = self.scan_lock.lock().await;
        let projects = active_project_candidates(&app).await;
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

    /// `scan_once` for one known project, minus project enumeration (which
    /// needs `SettingsState`), so tests can drive a real scan.
    #[cfg(test)]
    pub(crate) async fn scan_project_under_scan_lock(
        &self,
        app: AppHandle,
        dir: PathBuf,
    ) -> Result<(), String> {
        let _guard = self.scan_lock.lock().await;
        self.scan_project(&app, &dir, false, false).await
    }

    #[cfg(test)]
    pub(crate) fn install_delivery_gate(&self, gate: LoopDeliveryGate) {
        *self.delivery_gate.lock().expect("delivery gate") = Some(gate);
    }

    async fn deliver(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        config: &LoopConfigToml,
        run_id: Uuid,
        due_at: DateTime<Utc>,
    ) -> LoopDeliveryReport {
        #[cfg(test)]
        let gate = self.delivery_gate.lock().expect("delivery gate").take();
        #[cfg(test)]
        if let Some(mut gate) = gate {
            if let Some(entered) = gate.entered.take() {
                let _ = entered.send(());
            }
            if let Some(release) = gate.release.take() {
                let _ = release.await;
            }
            return gate.report.take().expect("delivery gate report");
        }
        deliver_loop_prompt(app, project_dir, config, run_id, due_at).await
    }

    async fn scan_project(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        startup: bool,
        pending_only: bool,
    ) -> Result<(), String> {
        let Some(ac_root) = existing_ac_root(project_dir) else {
            return Ok(());
        };
        let loop_dirs = loop_dirs(&ac_root)?;
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
        // S0: a Loop that is gone at entry is a quiet skip.
        let (config, mut state, s0_raw, s0_generation) = {
            let _io = self.io_lock.lock().await;
            if !dir.is_dir() {
                return Ok(());
            }
            let Some(config) = read_loop_config_if_present(dir)? else {
                return Ok(());
            };
            let (state, raw) = read_state_snapshot(dir)?;
            (config, state, raw, self.loop_generation(dir))
        };
        if !config.loop_def.enabled {
            return Ok(());
        }
        let s0_raw = s0_raw.as_deref();

        if state.last_checked_at.is_none() {
            let _io = self.io_lock.lock().await;
            if !loop_is_current_for_delivery(dir, &config)? {
                return Ok(());
            }
            state = baseline_loop_state(&config, Utc::now())?;
            self.commit_scan_section(dir, s0_generation, s0_raw, None, &state)?;
            return Ok(());
        }

        if let (Some(pending_due), Some(pending_run)) = (state.pending_due_at, state.pending_run_id)
        {
            if !loop_is_current_for_delivery(dir, &config)? {
                return Ok(());
            }
            let started_at = Utc::now();
            let report = self
                .deliver(app, project_dir, &config, pending_run, pending_due)
                .await;
            if report.kind == LoopAuditKind::PendingBusy {
                self.maybe_coalesce_pending(
                    app,
                    project_dir,
                    dir,
                    &config,
                    &mut state,
                    s0_generation,
                    s0_raw,
                )
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
                s0_generation,
                s0_raw,
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
            self.record_missed_while_closed(
                app,
                project_dir,
                dir,
                &config,
                state,
                run_id,
                due_at,
                s0_generation,
                s0_raw,
            )
            .await?;
            return Ok(());
        }

        let started_at = Utc::now();
        if !loop_is_current_for_delivery(dir, &config)? {
            return Ok(());
        }
        let report = self
            .deliver(app, project_dir, &config, run_id, due_at)
            .await;
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
            s0_generation,
            s0_raw,
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn maybe_coalesce_pending(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        config: &LoopConfigToml,
        state: &mut LoopState,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
    ) -> Result<(), String> {
        let io = self.io_lock.lock().await;
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
        let entry = LoopAuditEntry {
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
            session_start: Some(config.policy.session_start),
            error: None,
            prompt_snapshot: None,
        };
        let written = self.commit_scan_section(dir, s0_generation, s0_raw, Some(&entry), state)?;
        drop(io);
        if written == LoopStateWrite::Stale {
            return Ok(());
        }
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
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
    ) -> Result<LoopState, String> {
        let io = self.io_lock.lock().await;
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
        let entry = LoopAuditEntry {
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
            session_start: Some(config.policy.session_start),
            error: None,
            prompt_snapshot: None,
        };
        let written = self.commit_scan_section(dir, s0_generation, s0_raw, Some(&entry), &state)?;
        drop(io);
        if written == LoopStateWrite::Stale {
            return Ok(state);
        }
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
        state: LoopState,
        report: LoopDeliveryReport,
        run_id: Uuid,
        due_at: DateTime<Utc>,
        started_at: DateTime<Utc>,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
    ) -> Result<LoopState, String> {
        let io = self.io_lock.lock().await;
        if !loop_is_current_for_delivery(dir, config)? {
            return Ok(state);
        }
        let mut next = state.clone();
        let now = Utc::now();
        next.last_checked_at = Some(now);
        next.last_due_at = Some(due_at);
        next.next_due_at = next_due_after(&config.trigger.expr, now)?;
        next.last_result = Some(LoopLastResult {
            kind: audit_kind_name(&report.kind).to_string(),
            message: report.message.clone(),
        });

        match report.kind {
            LoopAuditKind::Delivered => {
                next.last_delivered_at = report.completed_at.or(Some(now));
                next.pending_due_at = None;
                next.pending_run_id = None;
            }
            LoopAuditKind::PendingBusy => {
                next.pending_due_at = Some(due_at);
                next.pending_run_id = Some(run_id);
            }
            LoopAuditKind::SkippedBusy | LoopAuditKind::DeliveryFailed => {
                next.pending_due_at = None;
                next.pending_run_id = None;
            }
            LoopAuditKind::MissedWhileClosed | LoopAuditKind::CoalescedPending => {}
        }

        let entry = LoopAuditEntry {
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
            session_start: Some(config.policy.session_start),
            error: report.error.clone(),
            prompt_snapshot: report.prompt_snapshot.clone(),
        };
        let written = self.commit_scan_section(dir, s0_generation, s0_raw, Some(&entry), &next)?;
        drop(io);
        if written == LoopStateWrite::Stale {
            return Ok(state);
        }
        emit_transition(
            app,
            project_dir,
            dir,
            config,
            &next,
            audit_kind_event(&report.kind),
            Some(report.message),
        );
        Ok(next)
    }

    /// `false` when the Loop was replaced (generation) or its `state.json`
    /// changed (raw bytes, absent included) since the scan read it.
    fn scan_write_is_fresh(
        &self,
        dir: &Path,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
    ) -> Result<bool, String> {
        if self.loop_generation(dir) != s0_generation {
            return Ok(false);
        }
        Ok(read_raw_loop_state(dir)?.as_deref() == s0_raw)
    }

    /// The one commit order for every scan-side write, run under `io_lock`:
    /// (1) freshness, (2) guarded state write, (3) audit append only after a
    /// `Written`. So no audit row exists without its state write.
    fn commit_scan_section(
        &self,
        dir: &Path,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
        audit: Option<&LoopAuditEntry>,
        state: &LoopState,
    ) -> Result<LoopStateWrite, String> {
        if !self.scan_write_is_fresh(dir, s0_generation, s0_raw)? {
            log::debug!(
                "[loops] Skipping stale Loop scan write for {}: the Loop changed since the scan read it",
                dir.display()
            );
            return Ok(LoopStateWrite::Stale);
        }
        if guarded_state_write(dir, state, s0_raw)? == LoopStateWrite::Stale {
            log::debug!(
                "[loops] Skipping stale Loop scan write for {}: state or config changed at the write",
                dir.display()
            );
            return Ok(LoopStateWrite::Stale);
        }
        if let Some(entry) = audit {
            if let Err(e) = append_loop_audit_once(dir, entry) {
                if read_loop_config_if_present(dir)?.is_some() {
                    return Err(e);
                }
                log::debug!(
                    "[loops] Dropping audit row for deleted Loop {}: {}",
                    dir.display(),
                    e
                );
            }
        }
        Ok(LoopStateWrite::Written)
    }
}

/// Every scan-side state write: a Loop whose `config.toml` is gone is `Stale`
/// with nothing written, else a compare-and-swap against `s0_raw`.
fn guarded_state_write(
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
fn generation_key(dir: &Path) -> String {
    let path = match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
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
fn read_state_snapshot(dir: &Path) -> Result<(LoopState, Option<Vec<u8>>), String> {
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

fn read_raw_loop_state(dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = dir.join(LOOP_STATE_FILE);
    match std::fs::read(&path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read {}: {}", path.display(), e)),
    }
}

impl Default for LoopScheduler {
    fn default() -> Self {
        Self::new()
    }
}

async fn active_project_candidates(app: &AppHandle) -> Vec<ProjectResolution> {
    let (project_paths, archived) = {
        let settings = app.state::<SettingsState>();
        let s = settings.read().await;
        (s.project_paths.clone(), s.archived_project_paths.clone())
    };
    let projects = enumerate_registered_project_candidates(&project_paths);
    let archived_roots = sessions_persistence::normalize_project_roots(&archived);
    retain_unarchived_candidates(projects, &archived_roots)
}

fn unresolved_targets_in_project(project_dir: &Path) -> Vec<UnresolvedLoopTarget> {
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

fn loop_dirs(ac_root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(ac_root).map_err(|e| format!("Failed to read Project AC Root: {}", e))?;
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

fn loop_is_current_for_delivery(dir: &Path, expected: &LoopConfigToml) -> Result<bool, String> {
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
fn retain_unarchived_candidates(
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
    emit_loop_change(
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
    fn retain_unarchived_candidates_drops_a_candidate_under_an_archived_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archived = temp.path().join("archived");
        let active = temp.path().join("active");
        std::fs::create_dir_all(&archived).expect("archived");
        std::fs::create_dir_all(&active).expect("active");
        let roots = sessions_persistence::normalize_project_roots(&[archived
            .to_string_lossy()
            .to_string()]);
        let archived_candidate = ProjectResolution {
            path: archived.clone(),
            folder_name: "archived".to_string(),
            registered: true,
        };
        let active_candidate = ProjectResolution {
            path: active.clone(),
            folder_name: "active".to_string(),
            registered: true,
        };

        let filtered = retain_unarchived_candidates(
            vec![archived_candidate, active_candidate.clone()],
            &roots,
        );

        assert_eq!(filtered, vec![active_candidate]);
    }

    #[test]
    fn retain_unarchived_candidates_returns_input_unchanged_when_archived_list_is_empty() {
        let candidate = ProjectResolution {
            path: PathBuf::from("Z:/does/not/exist"),
            folder_name: "missing".to_string(),
            registered: true,
        };
        let input = vec![candidate.clone()];
        let ptr = input.as_ptr();
        let capacity = input.capacity();

        let filtered = retain_unarchived_candidates(input, &[]);

        assert_eq!(filtered, vec![candidate]);
        assert_eq!(
            filtered.as_ptr(),
            ptr,
            "empty archived roots must skip the into_iter/collect round trip"
        );
        assert_eq!(filtered.capacity(), capacity);
    }

    fn project_with_loop(config: &LoopConfigToml) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create ac root");
        write_loop_config(&ac_root, config).expect("write config");
        tmp
    }

    #[test]
    fn unresolved_targets_in_project_reports_a_missing_room() {
        let config = sample_config();
        let tmp = project_with_loop(&config);

        let alerts = unresolved_targets_in_project(tmp.path());

        assert_eq!(alerts.len(), 1, "one unresolved Loop target");
        let alert = &alerts[0];
        assert_eq!(alert.loop_id, config.loop_def.id);
        assert_eq!(alert.loop_name, config.loop_def.name);
        assert_eq!(alert.workgroup, config.target.workgroup);
        assert!(
            alert.error.starts_with("Room '") && alert.error.contains("not found in project"),
            "unexpected error: {}",
            alert.error
        );
    }

    #[test]
    fn unresolved_targets_in_project_skips_disabled_loops() {
        let mut config = sample_config();
        config.loop_def.enabled = false;
        let tmp = project_with_loop(&config);

        assert!(unresolved_targets_in_project(tmp.path()).is_empty());
    }

    #[test]
    fn unresolved_targets_in_project_reports_a_room_without_an_orchestrator() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        std::fs::create_dir_all(tmp.path().join(".ac").join(&config.target.workgroup))
            .expect("create room dir");

        let alerts = unresolved_targets_in_project(tmp.path());

        assert_eq!(alerts.len(), 1);
        assert!(
            alerts[0]
                .error
                .contains("has no identity-verified orchestrator"),
            "unexpected error: {}",
            alerts[0].error
        );
    }

    #[test]
    fn unresolved_targets_in_project_returns_empty_without_an_ac_root() {
        let tmp = tempfile::tempdir().expect("tempdir");

        assert!(unresolved_targets_in_project(tmp.path()).is_empty());
    }

    #[test]
    fn unresolved_targets_in_project_orders_by_loop_dir() {
        let mut first = sample_config();
        first.loop_def.id = "aaa-sync".to_string();
        let mut second = sample_config();
        second.loop_def.id = "zzz-sync".to_string();

        let tmp = project_with_loop(&second);
        let ac_root = tmp.path().join(".ac");
        write_loop_config(&ac_root, &first).expect("write first config");

        let alerts = unresolved_targets_in_project(tmp.path());

        assert_eq!(
            alerts
                .iter()
                .map(|a| a.loop_id.as_str())
                .collect::<Vec<_>>(),
            vec!["aaa-sync", "zzz-sync"]
        );
    }

    #[test]
    fn lib_registers_list_unresolved_loop_targets() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("read lib.rs");

        assert!(
            source.contains("commands::loops::list_unresolved_loop_targets"),
            "lib.rs must register list_unresolved_loop_targets"
        );
    }

    #[test]
    fn scan_once_calls_archived_candidate_filter() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/loops/scheduler.rs"
        ))
        .expect("read scheduler.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production scheduler source");
        let candidates = production
            .find("async fn active_project_candidates(app: &AppHandle) -> Vec<ProjectResolution> {")
            .expect("active_project_candidates definition");
        let body = &production[candidates..];
        let normalize = body
            .find("let archived_roots = sessions_persistence::normalize_project_roots(&archived);")
            .expect("archived root normalization");
        let retain = body
            .find("retain_unarchived_candidates(projects, &archived_roots)")
            .expect("retain_unarchived_candidates call");

        assert!(
            normalize < retain,
            "active_project_candidates must subtract archived candidates after normalizing archived roots"
        );
    }

    #[test]
    fn revalidate_loop_current_detects_deleted_disabled_and_changed_configs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = sample_config();
        let dir = write_loop_config(tmp.path(), &config).expect("write config");

        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("current"),
            LoopConfigRevalidation::Current
        );

        let mut renamed = config.clone();
        renamed.loop_def.name = "Renamed sync".to_string();
        write_loop_config(tmp.path(), &renamed).expect("write renamed config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("renamed"),
            LoopConfigRevalidation::Current
        );

        let mut retargeted = config.clone();
        retargeted.target.workgroup = "wg-2-dev-team".to_string();
        write_loop_config(tmp.path(), &retargeted).expect("write retargeted config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("retargeted"),
            LoopConfigRevalidation::Changed
        );

        let mut disabled = config.clone();
        disabled.loop_def.enabled = false;
        write_loop_config(tmp.path(), &disabled).expect("write disabled config");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("disabled"),
            LoopConfigRevalidation::Disabled
        );

        std::fs::remove_dir_all(&dir).expect("remove loop dir");
        assert_eq!(
            revalidate_loop_current(&dir, &config).expect("gone"),
            LoopConfigRevalidation::Gone
        );
    }

    /// AC-17 fixture - the only app these writers need. They reach
    /// `append_loop_audit_once`, `write_loop_state_atomic` and
    /// `emit_transition`, and `emit_loop_change` only calls `app.emit`, which
    /// is infallible here, so no managed state is required.
    fn audit_writer_app() -> tauri::App {
        crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build audit writer app")
    }

    /// AC-17 test 1 - the coalesced-pending writer records the run's real
    /// `sessionStart`. Every config field is set BEFORE `write_loop_config`:
    /// the writer revalidates against the file first, so a later edit would
    /// read `Changed` and no audit row would be written at all.
    #[tokio::test]
    async fn records_session_start_on_a_coalesced_pending_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let ac_root = project_dir.join(".ac");
        let mut config = sample_config();
        config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
        config.trigger.expr = "* * * * *".to_string();
        let dir = write_loop_config(&ac_root, &config).expect("write config");

        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();
        let mut state = LoopState {
            pending_due_at: Some(Utc::now() - chrono::Duration::minutes(30)),
            pending_run_id: Some(Uuid::new_v4()),
            ..LoopState::default()
        };

        scheduler
            .maybe_coalesce_pending(
                app.handle(),
                &project_dir,
                &dir,
                &config,
                &mut state,
                0,
                None,
            )
            .await
            .expect("coalesce pending");

        let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
            .expect("audit read");
        let last = content
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .expect("an audit row");
        let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
        assert_eq!(row["kind"], serde_json::json!("coalescedPending"));
        assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
    }

    /// AC-17 test 2 - the missed-while-closed writer records the real value.
    #[tokio::test]
    async fn records_session_start_on_a_missed_while_closed_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let ac_root = project_dir.join(".ac");
        let mut config = sample_config();
        config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
        let dir = write_loop_config(&ac_root, &config).expect("write config");

        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();

        scheduler
            .record_missed_while_closed(
                app.handle(),
                &project_dir,
                &dir,
                &config,
                LoopState::default(),
                Uuid::new_v4(),
                Utc::now() - chrono::Duration::hours(1),
                0,
                None,
            )
            .await
            .expect("record missed while closed");

        let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
            .expect("audit read");
        let last = content
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .expect("an audit row");
        let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
        assert_eq!(row["kind"], serde_json::json!("missedWhileClosed"));
        assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
    }

    /// AC-17 test 3 - the delivery-report writer records the real value. All
    /// seven `LoopDeliveryReport` fields are spelled because the struct has no
    /// `Default`; `completed_at: None` is deliberate.
    #[tokio::test]
    async fn records_session_start_on_a_delivery_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let ac_root = project_dir.join(".ac");
        let mut config = sample_config();
        config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
        let dir = write_loop_config(&ac_root, &config).expect("write config");

        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();
        let now = Utc::now();

        scheduler
            .apply_delivery_report(
                app.handle(),
                &project_dir,
                &dir,
                &config,
                LoopState::default(),
                LoopDeliveryReport {
                    kind: LoopAuditKind::Delivered,
                    message: "delivered".to_string(),
                    target: None,
                    session_id: None,
                    error: None,
                    prompt_snapshot: None,
                    completed_at: None,
                },
                Uuid::new_v4(),
                now,
                now,
                0,
                None,
            )
            .await
            .expect("apply delivery report");

        let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
            .expect("audit read");
        let last = content
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .expect("an audit row");
        let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
        assert_eq!(row["kind"], serde_json::json!("delivered"));
        assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
    }

    fn delivered_report() -> LoopDeliveryReport {
        LoopDeliveryReport {
            kind: LoopAuditKind::Delivered,
            message: "gated delivery".to_string(),
            target: None,
            session_id: None,
            error: None,
            prompt_snapshot: None,
            completed_at: Some(Utc::now()),
        }
    }

    /// #2695 scheduler-side fixture: a project with one Loop whose pending
    /// run makes the next scan deliver, then apply the report (S2).
    fn pending_loop_fixture() -> (tempfile::TempDir, PathBuf) {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        let state = LoopState {
            last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            pending_due_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            pending_run_id: Some(Uuid::new_v4()),
            ..LoopState::default()
        };
        crate::config::loops::write_loop_state_atomic(&dir, &state).expect("write state");
        (tmp, dir)
    }

    fn tmp_files_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("read loop dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    /// #2695 round 2 - a non-UTF-8 `state.json` is unreadable state, not a
    /// scan failure: the scan replaces it with a baseline, as before #2695.
    #[tokio::test]
    async fn scan_heals_a_non_utf8_state_file() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        let state_path = dir.join(LOOP_STATE_FILE);
        std::fs::write(&state_path, [0xff, 0xfe, b'{']).expect("write invalid state");
        let app = audit_writer_app();

        LoopScheduler::new()
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("an unreadable state is not a scan failure");

        let state = crate::config::loops::read_loop_state(&dir).expect("healed state parses");
        assert!(state.last_checked_at.is_some(), "a baseline replaced it");
    }

    /// #2695 round 3 - the non-UTF-8 arm of `guarded_state_write` is still a
    /// compare-and-swap: different invalid bytes on disk mean `Stale`.
    #[test]
    fn guarded_state_write_is_stale_when_non_utf8_bytes_changed() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        let state_path = dir.join(LOOP_STATE_FILE);
        std::fs::write(&state_path, [0xff, 0xfe, b'A']).expect("write bytes A");
        let (_, s0_raw) = read_state_snapshot(&dir).expect("snapshot");
        let changed = [0xff, 0xfe, b'B'];
        std::fs::write(&state_path, changed).expect("write bytes B");

        let result = guarded_state_write(&dir, &LoopState::default(), s0_raw.as_deref());

        assert_eq!(result, Ok(LoopStateWrite::Stale));
        assert_eq!(std::fs::read(&state_path).expect("state bytes"), changed);
    }

    /// #2695 T3 - the scan lock was split, not deleted: a second scan cannot
    /// start while the first is held inside a delivery.
    #[tokio::test]
    async fn a_second_scan_waits_while_the_first_is_inside_a_delivery() {
        let (tmp, dir) = pending_loop_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        scheduler.install_delivery_gate(LoopDeliveryGate {
            entered: Some(entered_tx),
            release: Some(release_rx),
            report: Some(delivered_report()),
        });
        let spawn_scan = || {
            let scheduler = Arc::clone(&scheduler);
            let app = app.handle().clone();
            let project = tmp.path().to_path_buf();
            tokio::spawn(async move { scheduler.scan_project_under_scan_lock(app, project).await })
        };
        let first = spawn_scan();
        entered_rx.await.expect("first scan entered the delivery");

        let mut second = spawn_scan();
        let started = std::time::Instant::now();
        let blocked = tokio::time::timeout(Duration::from_millis(200), &mut second).await;
        eprintln!(
            "T3: second scan still blocked after {:?}",
            started.elapsed()
        );
        assert!(blocked.is_err(), "the second scan must wait on scan_lock");

        release_tx.send(()).expect("release the gate");
        first.await.expect("join first").expect("first scan");
        second.await.expect("join second").expect("second scan");
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert!(
            state.pending_run_id.is_none(),
            "the first scan applied its report"
        );
    }

    /// #2695 T7 - a Loop gone at entry is a quiet skip with no write.
    #[tokio::test]
    async fn scan_loop_skips_a_missing_dir_or_config_quietly() {
        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let ac_root = tmp.path().join(".ac");

        let missing = loop_dir(&ac_root, "gone");
        scheduler
            .scan_loop(app.handle(), tmp.path(), &missing, false, false)
            .await
            .expect("a missing directory is Ok");
        assert!(!missing.exists(), "nothing recreated the missing directory");

        let dir = loop_dir(&ac_root, &config.loop_def.id);
        std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE))
            .expect("remove config");
        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("a missing config is Ok");
        assert_eq!(
            std::fs::read_dir(&dir).expect("read loop dir").count(),
            0,
            "no write attempted"
        );
    }

    /// #2695 T12 - the write-time presence recheck, isolated.
    #[test]
    fn guarded_state_write_is_stale_when_the_config_is_gone() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        crate::config::loops::write_loop_state_atomic(&dir, &LoopState::default())
            .expect("write state");
        std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE))
            .expect("remove config");
        let (_, raw) = read_loop_state_with_raw(&dir).expect("read state");
        let state = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };

        let result = guarded_state_write(&dir, &state, raw.as_deref().map(str::as_bytes));

        assert_eq!(result, Ok(LoopStateWrite::Stale));
        assert_eq!(
            read_raw_loop_state(&dir).expect("raw"),
            raw.map(String::into_bytes)
        );
        assert!(tmp_files_in(&dir).is_empty(), "no tmp file left");
    }

    /// #2695 T13 - the generation key resists a spelling alias.
    #[test]
    fn generation_key_resists_a_spelling_alias() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir_all(ac_root.join("x")).expect("sibling");
        let dir = loop_dir(&ac_root, &config.loop_def.id);
        let dir_name = dir.file_name().expect("loop dir name");
        let aliased = ac_root.join("x").join("..").join(dir_name);

        assert_eq!(generation_key(&dir), generation_key(&aliased));
        #[cfg(windows)]
        assert_eq!(
            generation_key(&dir),
            generation_key(Path::new(&dir.to_string_lossy().to_uppercase()))
        );

        let scheduler = LoopScheduler::new();
        let before = scheduler.loop_generation(&dir);
        scheduler.bump_loop_generation(&aliased);
        assert_ne!(scheduler.loop_generation(&dir), before);
    }

    /// #2695 T15 - the commit order, isolated: with the config gone, the
    /// state write refuses before any audit append is attempted.
    #[test]
    fn commit_scan_section_writes_nothing_when_the_config_is_gone() {
        let config = sample_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        crate::config::loops::write_loop_state_atomic(
            &dir,
            &LoopState {
                last_checked_at: Some(Utc::now() - chrono::Duration::hours(1)),
                ..LoopState::default()
            },
        )
        .expect("write state");
        let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
        std::fs::write(&audit_path, "{\"row\":1}\n").expect("write audit");
        std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE))
            .expect("remove config");

        let scheduler = LoopScheduler::new();
        let (_, s0_raw) = read_loop_state_with_raw(&dir).expect("read state");
        let s0_generation = scheduler.loop_generation(&dir);
        let now = Utc::now();
        let entry = LoopAuditEntry {
            run_id: Uuid::new_v4(),
            loop_id: config.loop_def.id.clone(),
            project_path: tmp.path().to_string_lossy().to_string(),
            kind: LoopAuditKind::Delivered,
            due_at: now,
            started_at: now,
            completed_at: Some(now),
            target: None,
            session_id: None,
            busy_coordinator_policy: config.policy.busy_coordinator.clone(),
            session_start: Some(config.policy.session_start),
            error: None,
            prompt_snapshot: None,
        };
        let state = LoopState {
            last_checked_at: Some(now),
            ..LoopState::default()
        };

        let result = scheduler.commit_scan_section(
            &dir,
            s0_generation,
            s0_raw.as_deref().map(str::as_bytes),
            Some(&entry),
            &state,
        );

        assert_eq!(result, Ok(LoopStateWrite::Stale));
        assert_eq!(
            read_raw_loop_state(&dir).expect("raw"),
            s0_raw.map(String::into_bytes)
        );
        assert_eq!(
            std::fs::read_to_string(&audit_path).expect("audit"),
            "{\"row\":1}\n"
        );
        assert!(tmp_files_in(&dir).is_empty(), "no tmp file left");
    }

    /// #2679 - a failed state write appends no audit row. Windows-only: the
    /// failure is forced with an open read handle on `state.json`, which
    /// blocks the replace with `os error 5` on Windows but not on Unix. The
    /// ordering contract is cross-platform; a Linux green is not coverage.
    #[cfg(windows)]
    #[test]
    fn issue_2679_a_failed_state_write_appends_no_audit_row() {
        let (tmp, dir) = pending_loop_fixture();
        let config = sample_config();
        let state_path = dir.join(LOOP_STATE_FILE);
        let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
        let scheduler = LoopScheduler::new();
        let now = Utc::now();
        let entry_for = |run_id| LoopAuditEntry {
            run_id,
            loop_id: config.loop_def.id.clone(),
            project_path: tmp.path().to_string_lossy().to_string(),
            kind: LoopAuditKind::Delivered,
            due_at: now,
            started_at: now,
            completed_at: Some(now),
            target: None,
            session_id: None,
            busy_coordinator_policy: config.policy.busy_coordinator.clone(),
            session_start: Some(config.policy.session_start),
            error: None,
            prompt_snapshot: None,
        };
        let state = LoopState {
            last_checked_at: Some(now),
            ..LoopState::default()
        };

        // Positive control: with no holder the same commit writes and audits.
        let s0_raw = read_raw_loop_state(&dir)
            .expect("raw")
            .expect("state exists");
        let s0_generation = scheduler.loop_generation(&dir);
        let control = scheduler.commit_scan_section(
            &dir,
            s0_generation,
            Some(&s0_raw),
            Some(&entry_for(Uuid::new_v4())),
            &state,
        );
        assert_eq!(control, Ok(LoopStateWrite::Written));
        assert!(audit_path.exists(), "control leg appends the audit row");
        std::fs::remove_file(&audit_path).expect("remove control audit");
        std::fs::write(&state_path, &s0_raw).expect("restore state");

        // Failure leg: hold state.json open past the replace-retry budget.
        let s0_raw = read_raw_loop_state(&dir)
            .expect("raw")
            .expect("state exists");
        let s0_generation = scheduler.loop_generation(&dir);
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .open(&state_path)
            .expect("holder");
        let result = scheduler.commit_scan_section(
            &dir,
            s0_generation,
            Some(&s0_raw),
            Some(&entry_for(Uuid::new_v4())),
            &state,
        );
        drop(holder);

        let err = result.expect_err("the held state write fails");
        assert!(err.starts_with("Failed to finalize Loop state"), "{err}");
        assert!(!audit_path.exists(), "no audit row without its state write");
        assert_eq!(read_raw_loop_state(&dir).expect("raw"), Some(s0_raw));
        assert!(tmp_files_in(&dir).is_empty(), "{:?}", tmp_files_in(&dir));
    }
}
