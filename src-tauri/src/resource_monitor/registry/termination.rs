use crate::resource_monitor::registry::has_access_denied_signature;
use crate::resource_monitor::registry::identity_unavailable_pid;
use crate::resource_monitor::registry::is_missing_root_cleanup_error;
use crate::resource_monitor::registry::is_placeholder_identity;
use crate::resource_monitor::registry::push_pending_identity_error;
use crate::resource_monitor::registry::unverifiable_process_error;
use crate::resource_monitor::registry::QuarantineRetryOutcome;
use crate::resource_monitor::registry::QuarantineRetrySkip;
use crate::resource_monitor::registry::ResourceMonitorState;
use crate::resource_monitor::types::ProcessIdentity;
use crate::resource_monitor::types::ResourceGroupState;
use crate::resource_monitor::types::ResourceKillReason;
use crate::resource_monitor::types::ResourceKillResult;
use crate::resource_monitor::types::TerminateOutcome;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::Instant;
use uuid::Uuid;

impl ResourceMonitorState {
    pub fn kill_group(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
    ) -> Result<ResourceKillResult, String> {
        self.kill_group_inner(session_id, reason, false, None)
    }

    /// #1151 - registry-only cleanup retry for an ORPHANED quarantined group: one whose
    /// public session row is gone, so `SelectionCoordinator::watchdog_resource_kill`
    /// returns `NoPublicSession` and no production caller can ever reach `kill_group`
    /// for it again. Without this the group's admission permit leaks permanently.
    ///
    /// It decides only WHETHER `kill_group_inner` runs, never what it concludes: the
    /// permit is still released only by a full verified cleanup pass. The absence of a
    /// session row is never an input to that decision.
    ///
    /// `expected_root` binds the retry to the registration the watchdog sampled, and is
    /// re-checked inside the same critical section that flips the group to `Terminating`.
    pub(crate) fn retry_orphaned_quarantine(
        &self,
        session_id: Uuid,
        expected_root: ProcessIdentity,
    ) -> Result<QuarantineRetryOutcome, String> {
        if !self.supports_process_tree_enforcement() {
            return Ok(QuarantineRetryOutcome::Skipped(
                QuarantineRetrySkip::Unsupported,
            ));
        }
        // Classify under the lock. A Quarantined row can never be removed from the map
        // (register_group's retain and prune_terminated_groups both filter on
        // Terminated), so a passing classification cannot be invalidated by a removal,
        // only by a concurrent state transition, which kill_group_inner reports honestly.
        {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "resource monitor lock poisoned")?;
            let Some(group) = inner.groups.get(&session_id) else {
                return Ok(QuarantineRetryOutcome::Skipped(
                    QuarantineRetrySkip::NotRegistered,
                ));
            };
            if !matches!(group.state, ResourceGroupState::Quarantined) {
                return Ok(QuarantineRetryOutcome::Skipped(
                    QuarantineRetrySkip::NotQuarantined(group.state),
                ));
            }
            if group.root_identity != expected_root {
                return Ok(QuarantineRetryOutcome::Skipped(
                    QuarantineRetrySkip::RootMismatch,
                ));
            }
        }
        let result = self.kill_group_inner(
            session_id,
            ResourceKillReason::Watchdog,
            false,
            Some(expected_root),
        )?;
        Ok(QuarantineRetryOutcome::Completed(result))
    }

    /// #1151 - true if this single group still counts toward the admission cap. Mirrors
    /// the `active_count` predicate for one group so callers never re-derive it. An
    /// absent group returns false: it cannot be occupying a slot. Returns false on an
    /// unsupported backend, matching `active_agent_groups()`.
    pub(crate) fn group_counts_toward_admission(&self, session_id: Uuid) -> bool {
        if !self.supports_process_tree_enforcement() {
            return false;
        }
        self.inner
            .lock()
            .map(|inner| {
                inner.groups.get(&session_id).is_some_and(|group| {
                    !matches!(group.state, ResourceGroupState::Terminated) || !group.permit_released
                })
            })
            .unwrap_or(false)
    }

    /// #632 B2a - shared kill implementation. `fresh_targets_only` controls only the
    /// target SEED, never the verification: when true (app-shutdown bulk cleanup) the
    /// accumulated `observed_processes` set is skipped and only the fresh `observe_tree`
    /// is targeted, so the reaper does not walk hundreds of stale-dead PIDs (each of
    /// which would force a full toolhelp snapshot). The Job Object kill has already
    /// terminated each jobbed session's live tree at shutdown (`kill_all_jobs()` runs
    /// before this path; jobs are created per child in `spawn_sync`, `pty/job.rs`), so
    /// the fresh set is normally empty; its survivors are job-less sessions' trees.
    ///
    /// #1151 - `expected_quarantined_root` is the same kind of knob for AUTHORITY rather
    /// than targets: a caller-supplied binding that narrows which registration this kill
    /// may act on. It is checked inside the SAME critical section that flips the group to
    /// `Terminating`, so a stale watchdog sample cannot act on a replacement row. Like
    /// `fresh_targets_only` it never weakens verification; it can only refuse to start.
    pub(super) fn kill_group_inner(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        fresh_targets_only: bool,
        expected_quarantined_root: Option<ProcessIdentity>,
    ) -> Result<ResourceKillResult, String> {
        let kill_started = Instant::now();
        let (root_identity, previous_targets) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "resource monitor lock poisoned")?;
            let Some(group) = inner.groups.get_mut(&session_id) else {
                return Ok(ResourceKillResult {
                    session_id: session_id.to_string(),
                    state: ResourceGroupState::Terminated,
                    killed_processes: Vec::new(),
                    quarantined: false,
                    message: "resource group not registered".to_string(),
                    blocked_by_security: false,
                    finalized: false,
                });
            };
            // #1151 - when a caller binds this kill to a specific quarantined
            // registration, verify state and root identity inside the SAME critical
            // section that flips to Terminating. A stale watchdog sample must never act
            // on a replacement row (the only way one can exist under the same UUID is
            // register_group's insert below). Placed before the `match` so the guard is
            // strictly stronger than the Terminating/Terminated early returns, and before
            // mark_ac_stop so a rejected retry never latches a stop attribution.
            if let Some(expected) = expected_quarantined_root {
                if !matches!(group.state, ResourceGroupState::Quarantined)
                    || group.root_identity != expected
                {
                    return Ok(ResourceKillResult {
                        session_id: session_id.to_string(),
                        state: group.state,
                        killed_processes: Vec::new(),
                        quarantined: matches!(group.state, ResourceGroupState::Quarantined),
                        message: "resource group quarantine retry guard rejected".to_string(),
                        blocked_by_security: false,
                        finalized: false,
                    });
                }
            }
            match group.state {
                ResourceGroupState::Terminating => {
                    return Ok(ResourceKillResult {
                        session_id: session_id.to_string(),
                        state: ResourceGroupState::Terminating,
                        killed_processes: Vec::new(),
                        quarantined: false,
                        message: "resource group is already terminating".to_string(),
                        blocked_by_security: false,
                        finalized: false,
                    });
                }
                ResourceGroupState::Terminated => {
                    return Ok(ResourceKillResult {
                        session_id: session_id.to_string(),
                        state: ResourceGroupState::Terminated,
                        killed_processes: Vec::new(),
                        quarantined: false,
                        message: "resource group already terminated".to_string(),
                        blocked_by_security: false,
                        finalized: false,
                    });
                }
                ResourceGroupState::Running | ResourceGroupState::Quarantined => {}
            }
            group.state = ResourceGroupState::Terminating;
            group.kill_started_at = Some(Instant::now());
            (
                group.root_identity,
                if fresh_targets_only {
                    // #632 B2a - at shutdown, ignore the accumulated set entirely; target only the
                    // fresh live tree captured below. It is normally empty because the Job Object
                    // kill has already terminated each jobbed session's tree (lib.rs fires
                    // kill_all_jobs() before this path runs; jobs are created per child at spawn,
                    // pty/job.rs), so this fresh walk exists to catch job-less survivors and is
                    // the only source of targets on this path. Targets commonly resolve
                    // AlreadyGone here; killed=0 is the expected, correct record (#1438).
                    Vec::new()
                } else {
                    group
                        .observed_processes
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                },
            )
        };

        // #942 - we are committed to killing and have touched no process yet: tag the
        // stop as AC-initiated so the PTY child-exit event can never read it as a child
        // that died on its own. Deliberately AFTER the early returns above (a group that
        // is not registered, already terminating or already terminated kills nothing, so
        // it must not latch an attribution). No-op for sessions with no local spawn
        // record. Taken with no resource-monitor lock held.
        crate::pty::spawn_diagnostics::mark_ac_stop(session_id, reason.as_log_reason(), None);

        let mut targets = previous_targets;
        let mut current_cleanup_identities = BTreeSet::new();
        let mut pending_identity_errors = Vec::new();
        let mut errors = Vec::new();
        let observe_started = Instant::now();
        let observed_tree = self.backend.observe_tree(root_identity);
        let observe_ms = observe_started.elapsed().as_millis();
        match observed_tree {
            Ok(tree) => {
                let current_processes = tree.processes.clone();
                current_cleanup_identities
                    .extend(current_processes.iter().map(|process| process.identity));
                let mut ignored_root_errors = Vec::new();
                for error in &tree.errors {
                    if is_missing_root_cleanup_error(error, root_identity) {
                        ignored_root_errors.push(error.clone());
                        continue;
                    }
                    if let Some(pid) = identity_unavailable_pid(error) {
                        let matching_identity = current_processes
                            .iter()
                            .find(|process| process.identity.pid == pid)
                            .or_else(|| targets.iter().find(|process| process.identity.pid == pid))
                            .map(|process| process.identity);
                        if let Some(identity) = matching_identity {
                            push_pending_identity_error(
                                &mut pending_identity_errors,
                                identity,
                                error.clone(),
                            );
                            continue;
                        }
                    }
                    errors.push(error.clone());
                }
                if !tree.errors.is_empty() && errors.is_empty() && !ignored_root_errors.is_empty() {
                    log::debug!(
                        "[resource-monitor] cleanup sample for session \
                            {} had ignored root-missing error: {}",
                        session_id,
                        ignored_root_errors.join("; ")
                    );
                }
                targets.extend(current_processes);
                let _ = self.merge_tree(session_id, tree);
            }
            Err(err) => {
                errors.push(err.to_string());
            }
        }

        let mut unique = BTreeMap::new();
        for process in targets {
            unique.insert(process.identity, process);
        }
        let mut targets = unique.into_values().collect::<Vec<_>>();
        targets.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then_with(|| b.identity.cmp(&a.identity))
        });
        let target_count = targets.len();

        let mut killed = Vec::new();
        for process in targets.iter() {
            if !process.kill_allowed {
                if current_cleanup_identities.contains(&process.identity) {
                    push_pending_identity_error(
                        &mut pending_identity_errors,
                        process.identity,
                        unverifiable_process_error(process.identity.pid),
                    );
                    continue;
                }
                match self.backend.observe_identity(process.identity.pid) {
                    Ok(None) => {}
                    Ok(Some(identity))
                        if !is_placeholder_identity(process.identity)
                            && identity != process.identity => {}
                    Ok(Some(_)) => push_pending_identity_error(
                        &mut pending_identity_errors,
                        process.identity,
                        unverifiable_process_error(process.identity.pid),
                    ),
                    Err(err) => push_pending_identity_error(
                        &mut pending_identity_errors,
                        process.identity,
                        format!("pid {}: {err}", process.identity.pid),
                    ),
                }
                continue;
            }
            match self.backend.terminate_verified(process) {
                Ok(TerminateOutcome::Terminated) => {
                    killed.push(process.identity);
                    log::info!(
                        "[resource-monitor] killed pid={} session={} reason={}",
                        process.identity.pid,
                        session_id,
                        reason.as_log_reason()
                    );
                }
                Ok(TerminateOutcome::AlreadyGone) => {}
                Err(err) => push_pending_identity_error(
                    &mut pending_identity_errors,
                    process.identity,
                    format!("pid {}: {err}", process.identity.pid),
                ),
            }
        }

        for (identity, error) in pending_identity_errors {
            match self.backend.observe_identity(identity.pid) {
                Ok(None) => {
                    log::debug!(
                        "[resource-monitor] ignored cleanup identity error \
                            for gone pid={} session={}",
                        identity.pid,
                        session_id
                    );
                }
                Ok(Some(current)) if !is_placeholder_identity(identity) && current != identity => {
                    log::debug!(
                        "[resource-monitor] ignored cleanup identity error \
                            for reused pid={} session={}",
                        identity.pid,
                        session_id
                    );
                }
                Ok(Some(_)) | Err(_) => {
                    // #647 D: per-PID diagnostic so future AV-block repros are
                    // conclusive from the log (the error already begins with `pid <n>:`).
                    log::warn!(
                        "[resource-monitor] kill-failed session={} reason={} error={}",
                        session_id,
                        reason.as_log_reason(),
                        error
                    );
                    errors.push(error);
                }
            }
        }

        let quarantined = !errors.is_empty();
        // #647 D: flag an AV/EDR ACCESS_DENIED (exact `win32 error 5`) so the UI can
        // add the exclusion guidance; the per-PID detail stays in `message`.
        let blocked_by_security = quarantined && has_access_denied_signature(&errors);
        // #1438 - a quarantined session-destroy is the evidence trail for the separate
        // locked-directory defect: dump the full deduped target list once, so a follow-up
        // ticket can name the holder instead of guessing. Gated to SessionDestroy because
        // retry_orphaned_quarantine forwards ResourceKillReason::Watchdog, so a stuck
        // group cannot multiply the line.
        if quarantined && matches!(reason, ResourceKillReason::SessionDestroy) {
            let tree = targets
                .iter()
                .map(|p| {
                    format!(
                        "pid={} exe={} ppid={:?} depth={} kill_allowed={}",
                        p.identity.pid, p.exe_name, p.parent_pid, p.depth, p.kill_allowed
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            log::warn!(
                "[resource-monitor] quarantine-tree session={} \
                    reason={} root_pid={} targets={}: {}",
                session_id,
                reason.as_log_reason(),
                root_identity.pid,
                targets.len(),
                tree
            );
        }
        let state = if quarantined {
            ResourceGroupState::Quarantined
        } else {
            ResourceGroupState::Terminated
        };
        let message = if quarantined {
            format!("resource group cleanup incomplete: {}", errors.join("; "))
        } else {
            "resource group terminated and verified".to_string()
        };

        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "resource monitor lock poisoned")?;
            if let Some(group) = inner.groups.get_mut(&session_id) {
                group.state = state;
                group.last_error = if quarantined {
                    Some(message.clone())
                } else {
                    None
                };
                if !quarantined {
                    group.permit_released = true;
                    group.terminated_at = Some(Instant::now());
                }
            }
        }

        log::info!(
            "[resource-monitor] kill_group session={} reason={} \
                observe_tree_ms={} total_ms={} targets={} killed={} \
                quarantined={}",
            session_id,
            reason.as_log_reason(),
            observe_ms,
            kill_started.elapsed().as_millis(),
            target_count,
            killed.len(),
            quarantined
        );

        Ok(ResourceKillResult {
            session_id: session_id.to_string(),
            state,
            killed_processes: killed,
            quarantined,
            message,
            blocked_by_security,
            // #647: kill_group only accounts/verifies; the command finalizes the
            // session (PTY teardown + Exited tile), so this is always false here.
            finalized: false,
        })
    }

    /// #632 - app-shutdown bulk cleanup. Uses fresh-tree-only targets
    /// (`kill_group_inner` with `fresh_targets_only = true`). Its only caller is the
    /// app-exit handler. Fresh-only avoids re-introducing the #632 hang: after the
    /// Job Object kill the live tree is empty, and walking the accumulated stale set
    /// here would pay one full toolhelp snapshot per dead PID. Per-session kills keep
    /// the union behavior, which stays bounded via the B2b sampling prune.
    pub fn kill_all_owned_groups(&self, reason: ResourceKillReason) -> Vec<ResourceKillResult> {
        let ids = self
            .inner
            .lock()
            .map(|inner| inner.groups.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        ids.into_iter()
            .filter_map(|id| self.kill_group_inner(id, reason, true, None).ok())
            .collect()
    }
}
