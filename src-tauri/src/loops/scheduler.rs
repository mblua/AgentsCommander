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
    acquire_loop_lock_for_dir, append_loop_audit_once, baseline_loop_state, details_from_parts,
    latest_due_between, loop_delivery_config_matches, loop_dir, next_due_after, read_loop_config,
    read_loop_config_if_present, read_loop_state_with_raw, resolve_loop_target,
    revalidate_loop_current, write_loop_state_atomic, write_loop_state_if_unchanged,
    LoopAuditEntry, LoopAuditKind, LoopConfigDetails, LoopConfigRevalidation, LoopConfigToml,
    LoopLastResult, LoopState, LoopStateWrite, LOOP_DIR_PREFIX, LOOP_LOCK_TIMEOUT, LOOP_STATE_FILE,
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

/// Test-only pause in a #2698 replay, before its `io_lock` wait: signals
/// `reached`, waits for `release`, then signals `at_lock` just before it
/// awaits `io_lock`, with no other await in between.
#[cfg(test)]
pub(crate) struct LoopReplayGate {
    pub(crate) reached: tokio::sync::oneshot::Sender<()>,
    pub(crate) release: tokio::sync::oneshot::Receiver<()>,
    pub(crate) at_lock: tokio::sync::oneshot::Sender<()>,
}

/// #2698: a run whose prompt was delivered but whose commit failed. A later
/// scan retries the commit instead of delivering the run again.
#[derive(Clone)]
struct UnrecordedDelivery {
    /// Loop generation at delivery (app-side edits).
    generation: u64,
    /// Config the prompt was delivered with (CLI-side and config-only edits).
    config: LoopConfigToml,
    /// The audit row that was not written.
    entry: LoopAuditEntry,
    /// Message for the transition event.
    report_message: String,
    /// The state that failed to commit.
    delivered: LoopState,
    /// S0 `state.json` bytes the failed commit started from.
    state_before: Option<Vec<u8>>,
    /// What a successful state write leaves on disk (`to_string_pretty`).
    state_after: Vec<u8>,
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
    /// delivery. Scope: in-process writers only. Other processes (the CLI in
    /// `cli/loop_cmd.rs`) are excluded by the per-Loop sidecar lock
    /// (`acquire_loop_lock`, #2682), taken inside `io_lock` by every section.
    io_lock: tokio::sync::Mutex<()>,
    /// Per-Loop generation, bumped by every Loop command under `io_lock`, so
    /// a scan can tell that the Loop it read was replaced underneath it.
    loop_generations: std::sync::Mutex<HashMap<String, u64>>,
    /// #2698: Delivered runs whose state write failed, keyed like
    /// `loop_generations`. A scan retries the write instead of delivering again.
    unrecorded_deliveries: std::sync::Mutex<HashMap<String, UnrecordedDelivery>>,
    #[cfg(test)]
    delivery_gate: std::sync::Mutex<Option<LoopDeliveryGate>>,
    #[cfg(test)]
    replay_gate: std::sync::Mutex<Option<LoopReplayGate>>,
}

impl LoopScheduler {
    pub fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            scan_lock: tokio::sync::Mutex::new(()),
            io_lock: tokio::sync::Mutex::new(()),
            loop_generations: std::sync::Mutex::new(HashMap::new()),
            unrecorded_deliveries: std::sync::Mutex::new(HashMap::new()),
            #[cfg(test)]
            delivery_gate: std::sync::Mutex::new(None),
            #[cfg(test)]
            replay_gate: std::sync::Mutex::new(None),
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
            let _loop_lock = acquire_loop_lock_for_dir(&dir, LOOP_LOCK_TIMEOUT)?;
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
    pub(crate) fn install_replay_gate(&self, gate: LoopReplayGate) {
        *self.replay_gate.lock().expect("replay gate") = Some(gate);
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
            let _loop_lock = acquire_loop_lock_for_dir(dir, LOOP_LOCK_TIMEOUT)?;
            if !dir.is_dir() {
                return Ok(());
            }
            let Some(config) = read_loop_config_if_present(dir)? else {
                return Ok(());
            };
            let (state, raw) = read_state_snapshot(dir)?;
            (config, state, raw, self.loop_generation(dir))
        };
        let s0_raw = s0_raw.as_deref();
        // A clone: the record stays in the map until the replay writes it, so
        // a scan cancelled inside the replay cannot lose it.
        if let Some(record) = self.peek_unrecorded_delivery(dir) {
            if !config.loop_def.enabled {
                self.take_unrecorded_delivery(dir);
                log::warn!(
                    "[loops] Dropping unrecorded delivery for {}: the Loop is disabled",
                    dir.display()
                );
            } else if record.generation != s0_generation
                || (s0_raw != record.state_before.as_deref()
                    && s0_raw != Some(&record.state_after[..]))
                || !loop_delivery_config_matches(&config, &record.config)
            {
                self.take_unrecorded_delivery(dir);
                log::warn!(
                    "[loops] Dropping unrecorded delivery for {}: the Loop or its state changed",
                    dir.display()
                );
            } else {
                return self
                    .retry_unrecorded_delivery(app, project_dir, dir, record, s0_generation, s0_raw)
                    .await;
            }
        }
        if !config.loop_def.enabled {
            return Ok(());
        }

        if state.last_checked_at.is_none() {
            let _io = self.io_lock.lock().await;
            // #2682: no Loop lock here. `commit_scan_section` takes it at entry
            // and re-checks freshness and config presence under it; holding it
            // here too would self-deadlock, since a second handle in this
            // process blocks on the same sidecar (T0).
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
        // #2733 - an unresolved target repeating the previous result is not a
        // new transition: no audit row, no event.
        let repeat_unresolved = report.kind == LoopAuditKind::TargetUnresolved
            && state.last_result.as_ref().map(|r| r.kind.as_str()) == Some("targetUnresolved");
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
            LoopAuditKind::SkippedBusy
            | LoopAuditKind::DeliveryFailed
            | LoopAuditKind::TargetUnresolved => {
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
        let audit = if repeat_unresolved {
            None
        } else {
            Some(&entry)
        };
        let written = match self.commit_scan_section(dir, s0_generation, s0_raw, audit, &next) {
            Ok(written) => written,
            Err(e) => {
                if report.kind == LoopAuditKind::Delivered {
                    self.record_unrecorded_delivery(
                        dir,
                        s0_generation,
                        config,
                        entry,
                        &report.message,
                        next,
                        s0_raw,
                    );
                }
                return Err(e);
            }
        };
        drop(io);
        if written == LoopStateWrite::Stale {
            return Ok(state);
        }
        self.take_unrecorded_delivery(dir);
        if repeat_unresolved {
            return Ok(next);
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

    #[allow(clippy::too_many_arguments)]
    fn record_unrecorded_delivery(
        &self,
        dir: &Path,
        generation: u64,
        config: &LoopConfigToml,
        entry: LoopAuditEntry,
        report_message: &str,
        delivered: LoopState,
        s0_raw: Option<&[u8]>,
    ) {
        // Unserializable state would fail the write the same way: keep nothing.
        let Ok(state_after) = serde_json::to_string_pretty(&delivered) else {
            return;
        };
        let record = UnrecordedDelivery {
            generation,
            config: config.clone(),
            entry,
            report_message: report_message.to_string(),
            delivered,
            state_before: s0_raw.map(<[u8]>::to_vec),
            state_after: state_after.into_bytes(),
        };
        self.keep_unrecorded_delivery(dir, record);
    }

    fn keep_unrecorded_delivery(&self, dir: &Path, record: UnrecordedDelivery) {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .insert(generation_key(dir), record);
    }

    fn peek_unrecorded_delivery(&self, dir: &Path) -> Option<UnrecordedDelivery> {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .get(&generation_key(dir))
            .cloned()
    }

    fn take_unrecorded_delivery(&self, dir: &Path) -> Option<UnrecordedDelivery> {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .remove(&generation_key(dir))
    }

    /// #2698: commit a delivered run's recorded state and audit row, with no
    /// new delivery. The caller proved the on-disk state is the one the
    /// failed commit started from or the one it wrote.
    async fn retry_unrecorded_delivery(
        &self,
        app: &AppHandle,
        project_dir: &Path,
        dir: &Path,
        record: UnrecordedDelivery,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
    ) -> Result<(), String> {
        #[cfg(test)]
        let gate = self.replay_gate.lock().expect("replay gate").take();
        #[cfg(test)]
        if let Some(gate) = gate {
            let _ = gate.reached.send(());
            let _ = gate.release.await;
            let _ = gate.at_lock.send(());
        }
        let io = self.io_lock.lock().await;
        let result = self.commit_scan_section_checked(
            dir,
            s0_generation,
            s0_raw,
            Some(&record.entry),
            &record.delivered,
            Some(&record.config),
        );
        drop(io);
        // Stale or Err: the record stays for the next scan.
        if result? == LoopStateWrite::Written {
            self.take_unrecorded_delivery(dir);
            emit_transition(
                app,
                project_dir,
                dir,
                &record.config,
                &record.delivered,
                "delivered",
                Some(record.report_message),
            );
        }
        Ok(())
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

    /// The one commit order for every scan-side write, run under `io_lock`
    /// and the cross-process Loop lock (#2682): (1) freshness, (2) guarded state write, (3) audit append only after a
    /// `Written`. So no audit row exists without its state write.
    fn commit_scan_section(
        &self,
        dir: &Path,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
        audit: Option<&LoopAuditEntry>,
        state: &LoopState,
    ) -> Result<LoopStateWrite, String> {
        self.commit_scan_section_checked(dir, s0_generation, s0_raw, audit, state, None)
    }

    /// `commit_scan_section`, plus (#2698) a delivery identity check under the
    /// Loop lock when `delivery_config` is set: a config edit that raced the
    /// caller's check makes the commit `Stale`.
    fn commit_scan_section_checked(
        &self,
        dir: &Path,
        s0_generation: u64,
        s0_raw: Option<&[u8]>,
        audit: Option<&LoopAuditEntry>,
        state: &LoopState,
        delivery_config: Option<&LoopConfigToml>,
    ) -> Result<LoopStateWrite, String> {
        let _loop_lock = acquire_loop_lock_for_dir(dir, LOOP_LOCK_TIMEOUT)?;
        if !self.scan_write_is_fresh(dir, s0_generation, s0_raw)? {
            log::debug!(
                "[loops] Skipping stale Loop scan write for {}: the Loop changed since the scan read it",
                dir.display()
            );
            return Ok(LoopStateWrite::Stale);
        }
        if let Some(config) = delivery_config {
            if revalidate_loop_current(dir, config)? != LoopConfigRevalidation::Current {
                log::debug!(
                    "[loops] Skipping stale Loop scan write for {}: the delivery config changed",
                    dir.display()
                );
                return Ok(LoopStateWrite::Stale);
            }
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
        LoopAuditKind::TargetUnresolved => "targetUnresolved",
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
        LoopAuditKind::TargetUnresolved => "unresolved",
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

        assert!(!audit_path.exists(), "no audit row without its state write");
        let err = result.expect_err("the held state write fails");
        assert!(err.starts_with("Failed to finalize Loop state"), "{err}");
        assert_eq!(read_raw_loop_state(&dir).expect("raw"), Some(s0_raw));
        assert!(tmp_files_in(&dir).is_empty(), "{:?}", tmp_files_in(&dir));
    }

    // #2682 T2b - the CAS window in `commit_scan_section`, under the Loop lock.

    const SCAN_CHILD_DIR_ENV: &str = "AC_2682_SCAN_CHILD_DIR";
    const SCAN_CHILD_TEST_FQN: &str = "loops::scheduler::tests::issue_2682_scan_commit_child";
    const T2B_EXPR: &str = "0 0 1 1 *";

    /// A project whose `wg-1-dev-team` target resolves, so a real update
    /// section passes validation, with one Loop and a baseline state.
    fn t2b_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let matrix = ac_root.join("_agent_tech-lead");
        let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
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
        let config = sample_config();
        let dir = write_loop_config(&ac_root, &config).expect("write config");
        let baseline = baseline_loop_state(&config, Utc::now() - chrono::Duration::hours(1))
            .expect("baseline");
        write_loop_state_atomic(&dir, &baseline).expect("write state");
        (tmp, ac_root, dir)
    }

    /// The scheduler-shaped writer: one scan commit from a fresh snapshot.
    fn t2b_scan_commit(dir: &Path) -> Result<LoopStateWrite, String> {
        let scheduler = LoopScheduler::new();
        let s0_raw = read_raw_loop_state(dir)?;
        let s0_generation = scheduler.loop_generation(dir);
        let state = LoopState {
            last_checked_at: Some(Utc::now()),
            ..LoopState::default()
        };
        scheduler.commit_scan_section(dir, s0_generation, s0_raw.as_deref(), None, &state)
    }

    fn t2b_update(
        tmp: &tempfile::TempDir,
        ac_root: &Path,
    ) -> std::thread::JoinHandle<Result<LoopState, String>> {
        let (project, root) = (tmp.path().to_path_buf(), ac_root.to_path_buf());
        std::thread::spawn(move || {
            crate::config::loops::update_loop_files(
                &project,
                &root,
                "daily-sync",
                crate::config::loops::LoopUpdatePatch {
                    expr: Some(T2B_EXPR.to_string()),
                    ..Default::default()
                },
            )
            .map(|(_, _, state)| state)
        })
    }

    fn let_it_finish_if_unblocked<T>(handle: &std::thread::JoinHandle<T>) {
        let started = std::time::Instant::now();
        while !handle.is_finished() && started.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The update's reset state is what is on disk: no lost update.
    fn assert_update_survived(dir: &Path, update: LoopState) {
        let on_disk = crate::config::loops::read_loop_state(dir).expect("state on disk");
        assert_eq!(
            (on_disk.last_checked_at, on_disk.next_due_at),
            (update.last_checked_at, update.next_due_at),
            "the scan commit clobbered the update's reset state"
        );
        assert_eq!(
            read_loop_config(dir).expect("config").trigger.expr,
            T2B_EXPR
        );
    }

    /// #2682 T2b, in-process - the scan commit stops between its CAS compare
    /// and its replace while an update runs. With the Loop lock the update
    /// waits and lands after; without it the commit clobbers the update.
    #[test]
    fn issue_2682_t2b_scan_commit_does_not_clobber_an_update() {
        let (tmp, ac_root, dir) = t2b_fixture();
        let armed = crate::config::loops::pause::arm("after_cas_compare", &dir);
        let commit_dir = dir.clone();
        let writer_a = std::thread::spawn(move || t2b_scan_commit(&commit_dir));
        armed
            .reached
            .recv_timeout(Duration::from_secs(10))
            .expect("the scan commit reaches the pause");
        let writer_b = t2b_update(&tmp, &ac_root);
        let_it_finish_if_unblocked(&writer_b);
        armed.release.send(()).expect("release the scan commit");
        assert_eq!(
            writer_a.join().expect("writer A"),
            Ok(LoopStateWrite::Written)
        );
        let update = writer_b.join().expect("writer B").expect("update section");
        assert_update_survived(&dir, update);
    }

    /// #2682 - the child half of T2b's cross-process leg: a scan commit that
    /// pauses (armed by environment) after its CAS compare. A no-op without
    /// the child-only environment.
    #[test]
    fn issue_2682_scan_commit_child() {
        let Some(dir) = std::env::var_os(SCAN_CHILD_DIR_ENV) else {
            return;
        };
        let written = t2b_scan_commit(Path::new(&dir)).expect("child scan commit");
        assert_eq!(written, LoopStateWrite::Written);
        println!("AC_2682_SCAN_CHILD_WRITTEN");
    }

    /// #2682 T2b, cross-process - the lost update #2682 names: the scan commit
    /// runs in a child process, the update in this one.
    #[test]
    fn issue_2682_t2b_cross_process_scan_commit_does_not_clobber_an_update() {
        use crate::config::loops::pause;
        let (tmp, ac_root, dir) = t2b_fixture();
        let rendezvous = tempfile::tempdir().expect("rendezvous");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .args([
                "--exact",
                SCAN_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(SCAN_CHILD_DIR_ENV, &dir)
            .env(pause::PAUSE_DIR_ENV, rendezvous.path())
            .env(pause::PAUSE_STAGE_ENV, "after_cas_compare")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn scan child");
        let ready = rendezvous.path().join(pause::READY_FILE);
        let started = std::time::Instant::now();
        while !ready.exists() {
            if started.elapsed() > Duration::from_secs(60)
                || child.try_wait().ok().flatten().is_some()
            {
                let _ = child.kill();
                let output = child.wait_with_output().expect("child output");
                panic!(
                    "scan child never paused:\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let writer_b = t2b_update(&tmp, &ac_root);
        let_it_finish_if_unblocked(&writer_b);
        std::fs::write(rendezvous.path().join(pause::RELEASE_FILE), b"go").expect("release");
        let started = std::time::Instant::now();
        while child.try_wait().expect("poll child").is_none() {
            if started.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                panic!("scan child exceeded its 60 s bound");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success()
                && stdout.contains(&format!("test {SCAN_CHILD_TEST_FQN} ..."))
                && stdout.contains("test result: ok. 1 passed; 0 failed")
                && stdout.contains("AC_2682_SCAN_CHILD_WRITTEN"),
            "scan child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let update = writer_b.join().expect("writer B").expect("update section");
        assert_update_survived(&dir, update);
    }

    // #2698 - a failed state write after delivery must not cause re-delivery.

    /// Never due in the test window (next: 2028-02-29).
    const NEVER_DUE_EXPR: &str = "0 0 29 2 *";

    /// Config-only rewrite: `state.json` is untouched.
    fn edit_config(dir: &Path, edit: impl FnOnce(&mut LoopConfigToml)) -> LoopConfigToml {
        let mut config = read_loop_config(dir).expect("config");
        edit(&mut config);
        write_loop_config(dir.parent().expect("ac root"), &config).expect("write config");
        config
    }

    fn pending_state() -> LoopState {
        LoopState {
            last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            pending_due_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            pending_run_id: Some(Uuid::new_v4()),
            ..LoopState::default()
        }
    }

    /// `pending_loop_fixture` with a never-due trigger.
    fn issue_2698_fixture() -> (tempfile::TempDir, PathBuf, Uuid) {
        let (tmp, dir) = pending_loop_fixture();
        edit_config(&dir, |c| c.trigger.expr = NEVER_DUE_EXPR.to_string());
        let run_id = crate::config::loops::read_loop_state(&dir)
            .expect("state")
            .pending_run_id
            .expect("pending run");
        (tmp, dir, run_id)
    }

    /// `t2b_fixture` (the CLI path resolves its target) seeded with a pending
    /// run and a never-due trigger.
    fn issue_2698_cli_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Uuid) {
        let (tmp, ac_root, dir) = t2b_fixture();
        edit_config(&dir, |c| c.trigger.expr = NEVER_DUE_EXPR.to_string());
        let state = pending_state();
        write_loop_state_atomic(&dir, &state).expect("write state");
        let run_id = state.pending_run_id.expect("pending run");
        (tmp, ac_root, dir, run_id)
    }

    fn audit_rows(dir: &Path) -> Vec<LoopAuditEntry> {
        let path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
        if !path.is_file() {
            return Vec::new();
        }
        std::fs::read_to_string(path)
            .expect("audit read")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("audit row"))
            .collect()
    }

    fn has_record(scheduler: &LoopScheduler, dir: &Path) -> bool {
        scheduler
            .unrecorded_deliveries
            .lock()
            .expect("records")
            .contains_key(&generation_key(dir))
    }

    fn recorded_entry(scheduler: &LoopScheduler, dir: &Path) -> LoopAuditEntry {
        scheduler.unrecorded_deliveries.lock().expect("records")[&generation_key(dir)]
            .entry
            .clone()
    }

    fn gate_is_untaken(scheduler: &LoopScheduler) -> bool {
        scheduler.delivery_gate.lock().expect("gate").is_some()
    }

    fn install_report_gate(scheduler: &LoopScheduler, report: LoopDeliveryReport) {
        scheduler.install_delivery_gate(LoopDeliveryGate {
            entered: None,
            release: None,
            report: Some(report),
        });
    }

    /// T1 steps 2-3: a direct `scan_loop` that delivers `report`, then finds
    /// the Loop lock held by the test and fails its commit.
    async fn scan_with_the_loop_lock_held(
        scheduler: &Arc<LoopScheduler>,
        app: &tauri::App,
        project: &Path,
        dir: &Path,
        report: LoopDeliveryReport,
    ) -> Result<(), String> {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        scheduler.install_delivery_gate(LoopDeliveryGate {
            entered: Some(entered_tx),
            release: Some(release_rx),
            report: Some(report),
        });
        let scan = {
            let scheduler = Arc::clone(scheduler);
            let app = app.handle().clone();
            let (project, dir) = (project.to_path_buf(), dir.to_path_buf());
            tokio::spawn(async move {
                scheduler
                    .scan_loop(&app, &project, &dir, false, false)
                    .await
            })
        };
        entered_rx.await.expect("the scan entered the delivery");
        let lock = acquire_loop_lock_for_dir(dir, LOOP_LOCK_TIMEOUT).expect("test Loop lock");
        release_tx.send(()).expect("release the gate");
        let result = scan.await.expect("join scan");
        drop(lock);
        result
    }

    /// T1 steps 1-4, shared: returns the state bytes after the failed scan.
    async fn failed_first_scan(
        scheduler: &Arc<LoopScheduler>,
        app: &tauri::App,
        project: &Path,
        dir: &Path,
        run_id: Uuid,
    ) -> Vec<u8> {
        let before = read_raw_loop_state(dir).expect("raw").expect("state");
        let err = scan_with_the_loop_lock_held(scheduler, app, project, dir, delivered_report())
            .await
            .expect_err("the commit fails on the held Loop lock");
        assert!(err.contains("loopLockTimeout"), "{err}");
        let after = read_raw_loop_state(dir).expect("raw").expect("state");
        assert_eq!(after, before, "state.json unchanged");
        let state = crate::config::loops::read_loop_state(dir).expect("state");
        assert_eq!(state.pending_run_id, Some(run_id), "run still pending");
        assert!(audit_rows(dir).is_empty(), "no audit row");
        assert!(has_record(scheduler, dir), "one record under the key");
        after
    }

    fn assert_old_run_not_recorded(dir: &Path, run_id: Uuid) {
        assert!(
            audit_rows(dir).iter().all(|row| row.run_id != run_id),
            "the old delivery was recorded against a changed Loop"
        );
        let state = crate::config::loops::read_loop_state(dir).expect("state");
        assert_eq!(state.last_delivered_at, None);
        assert_ne!(
            state.last_result.map(|r| r.kind),
            Some("delivered".to_string())
        );
    }

    #[tokio::test]
    async fn issue_2698_failed_write_after_delivery_is_retried_not_redelivered() {
        let (tmp, dir, run_id) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;

        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(gate_is_untaken(&scheduler), "the second scan re-delivered");
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert_eq!(state.pending_run_id, None);
        assert!(state.last_delivered_at.is_some());
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, LoopAuditKind::Delivered);
        assert_eq!(rows[0].run_id, run_id);
        assert!(!has_record(&scheduler, &dir), "record cleared");

        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("third scan");
        assert!(gate_is_untaken(&scheduler), "the third scan re-delivered");
        assert_eq!(audit_rows(&dir).len(), 1);
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_when_the_loop_generation_changes() {
        let (tmp, dir, run_id) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
        scheduler.bump_loop_generation(&dir);

        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!gate_is_untaken(&scheduler), "normal scan delivers");
        assert!(!has_record(&scheduler, &dir));
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_after_a_cli_edit() {
        let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        let failed = failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
        let entry = recorded_entry(&scheduler, &dir);
        let generation = scheduler.loop_generation(&dir);

        crate::config::loops::update_loop_files(
            tmp.path(),
            &ac_root,
            "daily-sync",
            crate::config::loops::LoopUpdatePatch {
                prompt_body: Some("edited".into()),
                ..Default::default()
            },
        )
        .expect("CLI update");
        assert_eq!(scheduler.loop_generation(&dir), generation);
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert_eq!(state.pending_run_id, None, "reset to a baseline");
        assert_ne!(read_raw_loop_state(&dir).expect("raw"), Some(failed));

        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!has_record(&scheduler, &dir));
        assert!(audit_rows(&dir)
            .iter()
            .all(|row| row.run_id != entry.run_id && row.started_at != entry.started_at));
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_after_a_config_only_edit() {
        let (tmp, dir, run_id) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        let failed = failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
        let entry = recorded_entry(&scheduler, &dir);
        let generation = scheduler.loop_generation(&dir);

        edit_config(&dir, |c| c.prompt.body = "edited".to_string());
        assert_eq!(scheduler.loop_generation(&dir), generation);
        assert_eq!(read_raw_loop_state(&dir).expect("raw"), Some(failed));
        assert!(audit_rows(&dir).iter().all(|row| row.run_id != run_id));

        let t2 = Utc::now();
        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!has_record(&scheduler, &dir));
        assert!(
            !gate_is_untaken(&scheduler),
            "the pending run is delivered anew"
        );
        let rows = audit_rows(&dir);
        let delivered: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == LoopAuditKind::Delivered && row.run_id == run_id)
            .collect();
        assert_eq!(delivered.len(), 1, "{rows:?}");
        assert!(delivered[0].started_at >= t2);
        assert_ne!(delivered[0].started_at, entry.started_at);
        assert!(rows.iter().all(|row| row.started_at != entry.started_at));
    }

    #[tokio::test]
    async fn issue_2698_audit_failure_after_state_write_is_replayed() {
        let (tmp, dir, run_id) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();
        let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
        std::fs::create_dir(&audit_path).expect("audit.jsonl as a directory");

        install_report_gate(&scheduler, delivered_report());
        let err = scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect_err("the audit append fails");
        assert!(err.contains("Failed to read"), "{err}");
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert_eq!(state.pending_run_id, None, "the state was written");
        assert!(has_record(&scheduler, &dir));

        std::fs::remove_dir(&audit_path).expect("remove audit dir");
        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(gate_is_untaken(&scheduler), "the second scan re-delivered");
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, LoopAuditKind::Delivered);
        assert_eq!(rows[0].run_id, run_id);
        assert!(!has_record(&scheduler, &dir));

        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("third scan");
        assert!(gate_is_untaken(&scheduler));
        assert_eq!(audit_rows(&dir).len(), 1);
    }

    #[tokio::test]
    async fn issue_2698_checked_commit_is_stale_when_config_changed_under_lock() {
        let (_tmp, dir, _) = issue_2698_fixture();
        let scheduler = LoopScheduler::new();
        let old_config = read_loop_config(&dir).expect("config");
        let (state, raw) = read_state_snapshot(&dir).expect("snapshot");
        let generation = scheduler.loop_generation(&dir);
        edit_config(&dir, |c| c.prompt.body = "edited".to_string());

        let checked = scheduler.commit_scan_section_checked(
            &dir,
            generation,
            raw.as_deref(),
            None,
            &state,
            Some(&old_config),
        );
        assert_eq!(checked, Ok(LoopStateWrite::Stale));
        assert_eq!(read_raw_loop_state(&dir).expect("raw"), raw);

        let unchecked = scheduler.commit_scan_section_checked(
            &dir,
            generation,
            raw.as_deref(),
            None,
            &state,
            None,
        );
        assert_eq!(unchecked, Ok(LoopStateWrite::Written));
    }

    #[tokio::test]
    async fn issue_2698_non_delivered_failure_keeps_old_behavior() {
        let (tmp, dir, _) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        let failed = LoopDeliveryReport {
            kind: LoopAuditKind::DeliveryFailed,
            message: "gated failure".to_string(),
            error: Some("gated failure".to_string()),
            ..delivered_report()
        };
        let err = scan_with_the_loop_lock_held(&scheduler, &app, tmp.path(), &dir, failed)
            .await
            .expect_err("the commit fails on the held Loop lock");
        assert!(err.contains("loopLockTimeout"), "{err}");
        assert!(!has_record(&scheduler, &dir));

        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!gate_is_untaken(&scheduler), "the run is delivered again");
    }

    async fn issue_2698_cli_toggle_case(scan_between: bool) {
        let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
        let toggle = |enabled| {
            crate::config::loops::update_loop_files(
                tmp.path(),
                &ac_root,
                "daily-sync",
                crate::config::loops::LoopUpdatePatch {
                    enabled: Some(enabled),
                    ..Default::default()
                },
            )
            .expect("CLI toggle")
        };
        toggle(false);
        if scan_between {
            scheduler
                .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
                .await
                .expect("disabled scan");
            assert!(!has_record(&scheduler, &dir), "a disabled scan drops it");
        }
        toggle(true);
        let reenabled = crate::config::loops::read_loop_state(&dir).expect("state");

        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!has_record(&scheduler, &dir));
        assert_old_run_not_recorded(&dir, run_id);
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert_eq!(
            (state.last_checked_at, state.next_due_at),
            (reenabled.last_checked_at, reenabled.next_due_at)
        );
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_after_cli_disable_and_reenable() {
        issue_2698_cli_toggle_case(true).await;
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_after_cli_disable_and_reenable_without_a_scan() {
        issue_2698_cli_toggle_case(false).await;
    }

    #[tokio::test]
    async fn issue_2698_record_is_dropped_after_prompt_edit_away_and_back() {
        let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
        let recorded = scheduler.unrecorded_deliveries.lock().expect("records")
            [&generation_key(&dir)]
            .config
            .clone();
        let generation = scheduler.loop_generation(&dir);
        for body in ["edited".to_string(), recorded.prompt.body.clone()] {
            crate::config::loops::update_loop_files(
                tmp.path(),
                &ac_root,
                "daily-sync",
                crate::config::loops::LoopUpdatePatch {
                    prompt_body: Some(body),
                    ..Default::default()
                },
            )
            .expect("CLI update");
        }
        assert!(loop_delivery_config_matches(
            &read_loop_config(&dir).expect("config"),
            &recorded
        ));
        assert_eq!(scheduler.loop_generation(&dir), generation);
        let reset = crate::config::loops::read_loop_state(&dir).expect("state");

        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("second scan");
        assert!(!has_record(&scheduler, &dir));
        assert_old_run_not_recorded(&dir, run_id);
        let state = crate::config::loops::read_loop_state(&dir).expect("state");
        assert_eq!(
            (state.last_checked_at, state.next_due_at),
            (reset.last_checked_at, reset.next_due_at)
        );
    }

    /// Rework 1/2 - a replay cancelled while it waits for `io_lock` keeps
    /// its record: the next scan still replays and does not deliver again.
    /// The replay gate makes the order deterministic: the test takes
    /// `io_lock` while the scan is parked in the gate, and `at_lock` arrives
    /// only once the scan's task yields, which (current-thread runtime) is
    /// at the `io_lock` wait, since no other await follows the gate.
    #[tokio::test]
    async fn issue_2698_cancelled_replay_keeps_the_record() {
        assert_eq!(
            tokio::runtime::Handle::current().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread
        );
        let (tmp, dir, run_id) = issue_2698_fixture();
        let app = audit_writer_app();
        let scheduler = Arc::new(LoopScheduler::new());
        failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;

        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (at_lock_tx, at_lock_rx) = tokio::sync::oneshot::channel();
        scheduler.install_replay_gate(LoopReplayGate {
            reached: reached_tx,
            release: release_rx,
            at_lock: at_lock_tx,
        });
        let scan = {
            let scheduler = Arc::clone(&scheduler);
            let app = app.handle().clone();
            let (project, dir) = (tmp.path().to_path_buf(), dir.clone());
            tokio::spawn(async move {
                scheduler
                    .scan_loop(&app, &project, &dir, false, false)
                    .await
            })
        };
        reached_rx.await.expect("the scan reached the replay");
        let held = scheduler.io_lock.lock().await;
        release_tx.send(()).expect("release the replay gate");
        at_lock_rx.await.expect("the replay waits for io_lock");
        assert!(!scan.is_finished(), "the replay is parked on io_lock");
        scan.abort();
        assert!(scan.await.expect_err("aborted").is_cancelled());
        drop(held);
        assert!(
            has_record(&scheduler, &dir),
            "a cancelled replay lost its record"
        );

        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("next scan");
        assert!(gate_is_untaken(&scheduler), "the next scan re-delivered");
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].run_id, run_id);
        assert!(!has_record(&scheduler, &dir));
    }

    // #2733 - unresolved-target transitions.

    fn issue_2733_due_config() -> LoopConfigToml {
        let mut config = sample_config();
        config.trigger.expr = "* * * * *".to_string();
        config
    }

    fn issue_2733_make_due(dir: &Path) {
        let mut state = crate::config::loops::read_loop_state(dir).unwrap_or_default();
        state.last_checked_at = Some(Utc::now() - chrono::Duration::minutes(5));
        write_loop_state_atomic(dir, &state).expect("write due state");
    }

    fn issue_2733_state(dir: &Path) -> LoopState {
        crate::config::loops::read_loop_state(dir).expect("read state")
    }

    fn issue_2733_last_kind(dir: &Path) -> Option<String> {
        issue_2733_state(dir).last_result.map(|r| r.kind)
    }

    fn issue_2733_listen(app: &tauri::App) -> Arc<std::sync::Mutex<Vec<String>>> {
        use tauri::Listener;
        let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&kinds);
        app.listen_any("loop_event", move |event| {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
            if let Some(kind) = payload["kind"].as_str() {
                sink.lock().expect("events").push(kind.to_string());
            }
        });
        kinds
    }

    fn issue_2733_count(kinds: &Arc<std::sync::Mutex<Vec<String>>>, kind: &str) -> usize {
        kinds
            .lock()
            .expect("events")
            .iter()
            .filter(|k| *k == kind)
            .count()
    }

    fn issue_2733_create_room(project: &Path) {
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let matrix = ac_root.join("_agent_tech-lead");
        let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        for dir in [&team_dir, &matrix, &replica] {
            std::fs::create_dir_all(dir).expect("create room dir");
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
    }

    /// T-B2 + T-B6 - one audit row and one event per transition, none on repeats.
    #[tokio::test]
    async fn issue_2733_unresolved_tick_notifies_once() {
        let config = issue_2733_due_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        issue_2733_make_due(&dir);
        let app = audit_writer_app();
        let kinds = issue_2733_listen(&app);
        let scheduler = LoopScheduler::new();

        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan 1");
        let after_1 = issue_2733_state(&dir);
        assert_eq!(
            after_1.last_result.as_ref().map(|r| r.kind.as_str()),
            Some("targetUnresolved")
        );
        assert!(after_1.next_due_at.expect("next due") > Utc::now());
        assert_eq!(after_1.pending_due_at, None);
        assert_eq!(after_1.pending_run_id, None);
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, LoopAuditKind::TargetUnresolved);
        assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);

        issue_2733_make_due(&dir);
        let forced_check = issue_2733_state(&dir)
            .last_checked_at
            .expect("forced check");
        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan 2");
        let after_2 = issue_2733_state(&dir);
        assert_eq!(audit_rows(&dir).len(), 1, "audit rows still 1");
        assert_eq!(issue_2733_count(&kinds, "unresolved"), 1, "events still 1");
        assert!(after_2.last_checked_at.expect("checked") > forced_check);
        assert!(after_2.next_due_at.expect("next due") > Utc::now());
        assert_eq!(
            issue_2733_last_kind(&dir).as_deref(),
            Some("targetUnresolved")
        );
        assert!(!has_record(&scheduler, &dir));
    }

    /// T-B3 - unresolved ticks never write the synced config.
    #[tokio::test]
    async fn issue_2733_unresolved_ticks_never_touch_config() {
        let config = issue_2733_due_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        let config_path = dir.join(crate::config::loops::LOOP_CONFIG_FILE);
        let before = std::fs::read(&config_path).expect("config bytes");
        let app = audit_writer_app();
        let scheduler = LoopScheduler::new();

        for _ in 0..2 {
            issue_2733_make_due(&dir);
            scheduler
                .scan_loop(app.handle(), tmp.path(), &dir, false, false)
                .await
                .expect("scan");
        }

        assert_eq!(std::fs::read(&config_path).expect("config bytes"), before);
        assert!(read_loop_config(&dir).expect("config").loop_def.enabled);
    }

    /// T-B4 - a pending run whose room is missing is cleared.
    #[tokio::test]
    async fn issue_2733_pending_run_with_missing_room_is_cleared() {
        let (tmp, dir) = pending_loop_fixture();
        let app = audit_writer_app();

        LoopScheduler::new()
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan");

        let state = issue_2733_state(&dir);
        assert_eq!(state.pending_due_at, None);
        assert_eq!(state.pending_run_id, None);
        assert_eq!(
            issue_2733_last_kind(&dir).as_deref(),
            Some("targetUnresolved")
        );
    }

    /// T-B5 - dedupe keys on the previous kind, not on "ever seen".
    #[tokio::test]
    async fn issue_2733_transition_again_after_another_result() {
        let config = issue_2733_due_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        let state = LoopState {
            last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            last_result: Some(LoopLastResult {
                kind: "delivered".to_string(),
                message: "earlier".to_string(),
            }),
            ..LoopState::default()
        };
        write_loop_state_atomic(&dir, &state).expect("write state");
        let app = audit_writer_app();
        let kinds = issue_2733_listen(&app);

        LoopScheduler::new()
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan");

        assert_eq!(audit_rows(&dir).len(), 1);
        assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);
    }

    /// T-B8 - scheduler state machine: unresolved, room appears and the
    /// (gated) delivery is recorded, then loss notifies again.
    #[tokio::test]
    async fn issue_2733_recovers_when_room_appears() {
        let config = issue_2733_due_config();
        let tmp = project_with_loop(&config);
        let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
        issue_2733_make_due(&dir);
        let app = audit_writer_app();
        let kinds = issue_2733_listen(&app);
        let scheduler = LoopScheduler::new();

        // 1. Room absent, real delivery.
        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan 1");
        assert_eq!(
            issue_2733_last_kind(&dir).as_deref(),
            Some("targetUnresolved")
        );
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, LoopAuditKind::TargetUnresolved);
        assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);
        let check_1 = issue_2733_state(&dir).last_checked_at.expect("check 1");

        // 2. The room appears.
        issue_2733_create_room(tmp.path());
        assert!(resolve_loop_target(tmp.path(), &config).is_ok());

        // 3. Due again, delivery gated to a Delivered report.
        issue_2733_make_due(&dir);
        install_report_gate(&scheduler, delivered_report());
        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan 2");

        // 4.
        assert!(!gate_is_untaken(&scheduler), "delivery was attempted");
        assert_eq!(issue_2733_last_kind(&dir).as_deref(), Some("delivered"));
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind, LoopAuditKind::Delivered);
        assert_eq!(issue_2733_count(&kinds, "delivered"), 1);
        assert_eq!(kinds.lock().expect("events").len(), 2);
        let after_2 = issue_2733_state(&dir);
        let next_due = after_2.next_due_at.expect("next due");
        assert!(next_due > Utc::now());
        assert!(next_due > check_1);
        assert_eq!(after_2.pending_due_at, None);
        assert_eq!(after_2.pending_run_id, None);
        assert!(!has_record(&scheduler, &dir));

        // 5. Loss after recovery notifies again.
        std::fs::remove_dir_all(tmp.path().join(".ac").join("wg-1-dev-team")).expect("remove room");
        issue_2733_make_due(&dir);
        scheduler
            .scan_loop(app.handle(), tmp.path(), &dir, false, false)
            .await
            .expect("scan 3");
        let rows = audit_rows(&dir);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].kind, LoopAuditKind::TargetUnresolved);
        assert_eq!(issue_2733_count(&kinds, "unresolved"), 2);
    }
}
