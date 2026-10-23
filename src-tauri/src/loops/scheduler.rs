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
    LoopLastResult, LoopState, LoopStateWrite, LOOP_DIR_PREFIX, LOOP_LOCK_TIMEOUT,
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

#[derive(Clone)]
enum SchedulerContext {
    Native(AppHandle),
    #[cfg(test)]
    Headless {
        owner: SettingsState,
        // P43 preserves the six event arguments inline in this test-only recorder.
        #[allow(clippy::type_complexity)]
        transitions: Arc<
            std::sync::Mutex<
                Vec<(
                    PathBuf,
                    PathBuf,
                    String,
                    String,
                    Option<crate::config::loops::AcLoopSummary>,
                    Option<String>,
                )>,
            >,
        >,
    },
}

impl SchedulerContext {
    fn settings_owner(&self) -> SettingsState {
        match self {
            Self::Native(app) => Arc::clone(app.state::<SettingsState>().inner()),
            #[cfg(test)]
            Self::Headless { owner, .. } => Arc::clone(owner),
        }
    }
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
    #[cfg(test)]
    completed_scans: std::sync::atomic::AtomicUsize,
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
            #[cfg(test)]
            completed_scans: std::sync::atomic::AtomicUsize::new(0),
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
        self.start_with_context(SchedulerContext::Native(app), shutdown);
    }

    fn start_with_context(self: Arc<Self>, context: SchedulerContext, shutdown: ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            self.scan_once(&context, true, false).await;
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = shutdown.token().cancelled() => break,
                    _ = interval.tick() => self.scan_once(&context, false, false).await,
                    _ = self.notify.notified() => self.scan_once(&context, false, false).await,
                }
            }
        });
    }

    pub async fn on_session_idle(&self, app: AppHandle, _session_id: Uuid) {
        let context = SchedulerContext::Native(app);
        self.scan_once(&context, false, true).await;
    }

    pub async fn run_loop_now(
        &self,
        app: AppHandle,
        project_dir: PathBuf,
        loop_id: String,
    ) -> Result<LoopConfigDetails, String> {
        let context = SchedulerContext::Native(app);
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
            .deliver(&context, &project_dir, &config, run_id, due_at)
            .await;
        let state = self
            .apply_delivery_report(
                &context,
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

    async fn scan_once(&self, context: &SchedulerContext, startup: bool, pending_only: bool) {
        let _guard = self.scan_lock.lock().await;
        let owner = context.settings_owner();
        let projects = active_project_candidates_for_owner(&owner).await;
        for project in projects {
            if let Err(e) = self
                .scan_project(context, &project.path, startup, pending_only)
                .await
            {
                log::warn!(
                    "[loops] Scheduler scan failed for project {}: {}",
                    project.path.display(),
                    e
                );
            }
        }
        #[cfg(test)]
        self.completed_scans
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// `scan_once` for one known project, minus project enumeration (which
    /// needs `SettingsState`), so tests can drive a real scan.
    #[cfg(test)]
    pub(crate) async fn scan_project_under_scan_lock(
        &self,
        app: AppHandle,
        dir: PathBuf,
    ) -> Result<(), String> {
        let context = SchedulerContext::Native(app);
        let _guard = self.scan_lock.lock().await;
        self.scan_project(&context, &dir, false, false).await
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
        context: &SchedulerContext,
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
        match context {
            SchedulerContext::Native(app) => {
                deliver_loop_prompt(app, project_dir, config, run_id, due_at).await
            }
            #[cfg(test)]
            SchedulerContext::Headless { .. } => {
                panic!("headless scheduler delivery requires installed gate")
            }
        }
    }

    async fn scan_project(
        &self,
        context: &SchedulerContext,
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
                .scan_loop(context, project_dir, &dir, startup, pending_only)
                .await
            {
                log::warn!("[loops] Loop scan failed for {}: {}", dir.display(), e);
            }
        }
        Ok(())
    }

    async fn scan_loop(
        &self,
        context: &SchedulerContext,
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
                    .retry_unrecorded_delivery(
                        context,
                        project_dir,
                        dir,
                        record,
                        s0_generation,
                        s0_raw,
                    )
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
                .deliver(context, project_dir, &config, pending_run, pending_due)
                .await;
            if report.kind == LoopAuditKind::PendingBusy {
                self.maybe_coalesce_pending(
                    context,
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
                context,
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
                context,
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
            .deliver(context, project_dir, &config, run_id, due_at)
            .await;
        self.apply_delivery_report(
            context,
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
        context: &SchedulerContext,
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
            context,
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
        context: &SchedulerContext,
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
            context,
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
        context: &SchedulerContext,
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
            context,
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
        context: &SchedulerContext,
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
                context,
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
    let path = crate::config::loops::resolved_loop_state_path(dir);
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
    let owner = Arc::clone(app.state::<SettingsState>().inner());
    active_project_candidates_for_owner(&owner).await
}

async fn active_project_candidates_for_owner(owner: &SettingsState) -> Vec<ProjectResolution> {
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
            app,
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
mod tests;
