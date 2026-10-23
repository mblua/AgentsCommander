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
    acquire_loop_lock_for_dir, baseline_loop_state, details_from_parts, latest_due_between,
    loop_delivery_config_matches, loop_dir, read_loop_config_if_present, LoopAuditEntry,
    LoopAuditKind, LoopConfigDetails, LoopConfigToml, LoopState, LOOP_LOCK_TIMEOUT,
};
use crate::config::projects::ProjectResolution;
use crate::config::settings::SettingsState;
use crate::loops::delivery::{deliver_loop_prompt, LoopDeliveryReport};
use crate::shutdown::ShutdownSignal;

mod record;
mod state;

use state::{
    active_project_candidates_for_owner, generation_key, loop_dirs, loop_is_current_for_delivery,
    read_state_snapshot, unresolved_targets_in_project,
};

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

#[cfg(test)]
mod tests;
