use std::path::Path;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::state::{
    audit_kind_event, audit_kind_name, emit_transition, generation_key, guarded_state_write,
    loop_is_current_for_delivery, read_raw_loop_state,
};
use super::{LoopScheduler, SchedulerContext, UnrecordedDelivery};
use crate::config::loops::{
    acquire_loop_lock_for_dir, append_loop_audit_once, latest_due_between, next_due_after,
    read_loop_config_if_present, revalidate_loop_current, LoopAuditEntry, LoopAuditKind,
    LoopConfigRevalidation, LoopConfigToml, LoopLastResult, LoopState, LoopStateWrite,
    LOOP_LOCK_TIMEOUT,
};
use crate::loops::delivery::LoopDeliveryReport;

impl LoopScheduler {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn maybe_coalesce_pending(
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
    pub(super) async fn record_missed_while_closed(
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
    pub(super) async fn apply_delivery_report(
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
    pub(super) fn record_unrecorded_delivery(
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

    pub(super) fn keep_unrecorded_delivery(&self, dir: &Path, record: UnrecordedDelivery) {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .insert(generation_key(dir), record);
    }

    pub(super) fn peek_unrecorded_delivery(&self, dir: &Path) -> Option<UnrecordedDelivery> {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .get(&generation_key(dir))
            .cloned()
    }

    pub(super) fn take_unrecorded_delivery(&self, dir: &Path) -> Option<UnrecordedDelivery> {
        self.unrecorded_deliveries
            .lock()
            .expect("unrecorded deliveries")
            .remove(&generation_key(dir))
    }

    /// #2698: commit a delivered run's recorded state and audit row, with no
    /// new delivery. The caller proved the on-disk state is the one the
    /// failed commit started from or the one it wrote.
    pub(super) async fn retry_unrecorded_delivery(
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
    pub(super) fn scan_write_is_fresh(
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
    /// and the cross-process Loop lock (#2682): (1) freshness, (2) guarded state
    /// write, (3) audit append only after a `Written`. So no audit row exists
    /// without its state write.
    pub(super) fn commit_scan_section(
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
    pub(super) fn commit_scan_section_checked(
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
                "[loops] Skipping stale Loop scan write for {}: the Loop changed since the scan \
                    read it",
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
                "[loops] Skipping stale Loop scan write for {}: state or config changed at the \
                    write",
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
