use super::selection::{self, Selection};
use crate::cleanup_diagnostics::{operation::Operation, schema, store::ArmClaim};
use crate::resource_monitor::registry::has_access_denied_signature;
use crate::resource_monitor::registry::identity_unavailable_pid;
use crate::resource_monitor::registry::is_missing_root_cleanup_error;
use crate::resource_monitor::registry::is_placeholder_identity;
use crate::resource_monitor::registry::push_pending_identity_error;
use crate::resource_monitor::registry::unverifiable_process_error;
use crate::resource_monitor::registry::QuarantineRetryOutcome;
use crate::resource_monitor::registry::QuarantineRetrySkip;
use crate::resource_monitor::registry::ResourceMonitorState;
use crate::resource_monitor::types::ObservedProcess;
use crate::resource_monitor::types::ProcessIdentity;
use crate::resource_monitor::types::ResourceGroupState;
use crate::resource_monitor::types::ResourceKillReason;
use crate::resource_monitor::types::ResourceKillResult;
use crate::resource_monitor::types::TerminateOutcome;
use std::collections::BTreeSet;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) enum DiagnosticTerminationError {
    Ordinary(super::ResourceError),
    Evidence(schema::EvidenceError),
}

impl From<schema::EvidenceError> for DiagnosticTerminationError {
    fn from(error: schema::EvidenceError) -> Self {
        Self::Evidence(error)
    }
}

#[derive(Debug)]
// P04 consumes the metadata; P02's compatible adapter only needs the result.
#[allow(dead_code)]
pub(crate) enum CleanupFailure {
    Ordinary(String),
    EvidenceVeto {
        result: ResourceKillResult,
        operation_id: Uuid,
        stage: String,
        effects_started: bool,
        accounting_state: String,
        accounting_error: Option<String>,
    },
}

impl From<&str> for CleanupFailure {
    fn from(error: &str) -> Self {
        Self::Ordinary(error.into())
    }
}

impl From<String> for CleanupFailure {
    fn from(error: String) -> Self {
        Self::Ordinary(error)
    }
}

impl CleanupFailure {
    fn compatible(self) -> Result<ResourceKillResult, String> {
        match self {
            Self::Ordinary(error) => Err(error),
            Self::EvidenceVeto { result, .. } => Ok(result),
        }
    }
}

struct CleanupPass {
    binding: schema::Binding,
    previous_state: ResourceGroupState,
    previous_started: Option<Instant>,
    started: Instant,
    claim: Option<ArmClaim>,
}

impl ResourceMonitorState {
    pub fn kill_group(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
    ) -> Result<ResourceKillResult, String> {
        match self.kill_group_diagnostic(session_id, reason) {
            Ok(result) => Ok(result),
            Err(error) => error.compatible(),
        }
    }

    // Preserve the specified typed veto envelope, including accounting failure facts.
    #[allow(clippy::result_large_err)]
    pub(crate) fn kill_group_diagnostic(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
    ) -> Result<ResourceKillResult, CleanupFailure> {
        self.kill_group_diagnostic_inner(session_id, reason, false, None)
    }

    fn kill_group_inner(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        fresh_targets_only: bool,
        expected_root: Option<ProcessIdentity>,
    ) -> Result<ResourceKillResult, String> {
        match self.kill_group_diagnostic_inner(
            session_id,
            reason,
            fresh_targets_only,
            expected_root,
        ) {
            Ok(result) => Ok(result),
            Err(error) => error.compatible(),
        }
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
    #[allow(clippy::result_large_err)]
    fn kill_group_diagnostic_inner(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        fresh_targets_only: bool,
        expected_quarantined_root: Option<ProcessIdentity>,
    ) -> Result<ResourceKillResult, CleanupFailure> {
        let kill_started = Instant::now();
        let (root_identity, previous_targets, pass) = {
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
            let binding = schema::Binding {
                run_id: self.diagnostics.run_id(),
                session_id,
                root: Some(selection::identity(group.root_identity)),
                registration_generation: group.generation,
            };
            let claim = if !fresh_targets_only
                && !matches!(
                    reason,
                    ResourceKillReason::SpawnRollback | ResourceKillReason::AppShutdown
                ) {
                self.diagnostics.take_arm_metadata(&binding)
            } else {
                None
            };
            let pass = CleanupPass {
                binding,
                previous_state: group.state,
                previous_started: group.kill_started_at,
                started: kill_started,
                claim,
            };
            group.state = ResourceGroupState::Terminating;
            group.kill_started_at = Some(kill_started);
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
                pass,
            )
        };

        self.execute_group_cleanup(
            session_id,
            reason,
            root_identity,
            previous_targets,
            kill_started,
            pass,
        )
    }

    #[allow(clippy::result_large_err)]
    fn execute_group_cleanup(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        root_identity: ProcessIdentity,
        previous_targets: Vec<ObservedProcess>,
        kill_started: Instant,
        mut pass: CleanupPass,
    ) -> Result<ResourceKillResult, CleanupFailure> {
        let operation = match pass.claim.take() {
            Some(claim) => {
                let id = claim.operation_id();
                match self
                    .diagnostics
                    .open_operation(claim.with_reason(reason.as_log_reason()))
                {
                    Ok(operation) => Some(operation),
                    Err(error) => return Err(self.evidence_veto(&pass, id, error, Vec::new())),
                }
            }
            None => None,
        };
        // #942 - we are committed to killing and have touched no process yet: tag the
        // stop as AC-initiated so the PTY child-exit event can never read it as a child
        // that died on its own. Deliberately AFTER the early returns above (a group that
        // is not registered, already terminating or already terminated kills nothing, so
        // it must not latch an attribution). No-op for sessions with no local spawn
        // record. Taken with no resource-monitor lock held.
        crate::pty::spawn_diagnostics::mark_ac_stop(
            session_id,
            reason.as_log_reason(),
            None,
            self.spawn.as_ref(),
        );

        let mut targets = previous_targets.clone();
        let mut current_cleanup_identities = BTreeSet::new();
        let mut pending_identity_errors = Vec::new();
        let mut errors = Vec::new();
        let observe_ms = self.observe_cleanup_targets(
            session_id,
            root_identity,
            &mut targets,
            &mut current_cleanup_identities,
            &mut pending_identity_errors,
            &mut errors,
        );

        let fresh = targets.split_off(previous_targets.len());
        let mut selection = selection::select_targets(&previous_targets, &fresh);
        let mut veto = None;
        if let Some(operation) = operation.as_ref() {
            let roots = self
                .inner
                .lock()
                .map(|inner| other_roots(&inner, session_id))
                .unwrap_or_default();
            self.complete_selection_facts(&mut selection, &roots);
            veto = operation.selection(&selection.rows).err();
        }
        let targets = &selection.targets;
        let target_count = targets.len();
        let (killed, calls, call_error) = if veto.is_none() {
            self.terminate_cleanup_targets(
                session_id,
                reason,
                &selection,
                &current_cleanup_identities,
                &mut pending_identity_errors,
                operation.as_ref(),
            )
        } else {
            (Vec::new(), 0, None)
        };
        veto = veto.or(call_error);
        if let Some(error) = veto.as_mut() {
            error.effects_started |= calls > 0;
        }

        self.verify_cleanup_identity_errors(
            session_id,
            reason,
            pending_identity_errors,
            &mut errors,
        );

        if let Some(operation) = operation.as_ref() {
            let summary = schema::Summary {
                partial: veto.is_some() || !errors.is_empty(),
                evidence_gap: veto.is_some(),
                killed_processes: killed.len(),
                accounting: "proposed".into(),
                errors: errors
                    .iter()
                    .cloned()
                    .chain(veto.iter().map(ToString::to_string))
                    .collect(),
            };
            // Exactly one attempt, including when an earlier stage already vetoed.
            if let Err(mut error) = operation.finish(summary) {
                error.effects_started |= calls > 0;
                if veto.is_none() {
                    veto = Some(error);
                }
            }
            if let Some(error) = veto {
                return Err(self.evidence_veto(&pass, operation.id(), error, killed));
            }
        }
        let quarantined = !errors.is_empty();
        let (state, message, blocked_by_security) =
            self.record_cleanup_result(session_id, root_identity, reason, targets, &errors, &pass)?;

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

    fn record_cleanup_result(
        &self,
        session_id: Uuid,
        root_identity: ProcessIdentity,
        reason: ResourceKillReason,
        targets: &[ObservedProcess],
        errors: &[String],
        pass: &CleanupPass,
    ) -> Result<(ResourceGroupState, String, bool), String> {
        let quarantined = !errors.is_empty();
        // #647 D: flag an AV/EDR ACCESS_DENIED (exact `win32 error 5`) so the UI can
        // add the exclusion guidance; the per-PID detail stays in `message`.
        let blocked_by_security = quarantined && has_access_denied_signature(errors);
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
                if !pass.owns(group) {
                    return Err("resource cleanup registration/transition changed".into());
                }
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

        Ok((state, message, blocked_by_security))
    }

    fn observe_cleanup_targets(
        &self,
        session_id: Uuid,
        root_identity: ProcessIdentity,
        targets: &mut Vec<ObservedProcess>,
        current_cleanup_identities: &mut BTreeSet<ProcessIdentity>,
        pending_identity_errors: &mut Vec<(ProcessIdentity, String)>,
        errors: &mut Vec<String>,
    ) -> u128 {
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
                                pending_identity_errors,
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

        observe_ms
    }

    fn terminate_cleanup_targets(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        selection: &Selection,
        current_cleanup_identities: &BTreeSet<ProcessIdentity>,
        pending_identity_errors: &mut Vec<(ProcessIdentity, String)>,
        operation: Option<&Operation>,
    ) -> (Vec<ProcessIdentity>, usize, Option<schema::EvidenceError>) {
        let mut killed = Vec::new();
        let mut calls = 0;
        for (process, row) in selection.targets.iter().zip(&selection.rows) {
            if !process.kill_allowed {
                if current_cleanup_identities.contains(&process.identity) {
                    push_pending_identity_error(
                        pending_identity_errors,
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
                        pending_identity_errors,
                        process.identity,
                        unverifiable_process_error(process.identity.pid),
                    ),
                    Err(err) => push_pending_identity_error(
                        pending_identity_errors,
                        process.identity,
                        format!("pid {}: {err}", process.identity.pid),
                    ),
                }
                continue;
            }
            let outcome = if let Some(operation) = operation {
                match self
                    .backend
                    .terminate_verified_diagnostic(process, row, operation)
                {
                    Err(DiagnosticTerminationError::Evidence(mut error)) => {
                        error.effects_started |= calls > 0;
                        if matches!(
                            error.known_exit,
                            Some(schema::ExitObservation::Exited { .. })
                        ) {
                            match self.backend.observe_identity(process.identity.pid) {
                                Ok(None) => killed.push(process.identity),
                                Ok(Some(current)) if current != process.identity => {
                                    killed.push(process.identity);
                                }
                                _ => {}
                            }
                        }
                        return (killed, calls, Some(error));
                    }
                    Err(DiagnosticTerminationError::Ordinary(error)) => {
                        calls += 1;
                        Err(error)
                    }
                    Ok(outcome) => {
                        calls += 1;
                        Ok(outcome)
                    }
                }
            } else {
                self.backend.terminate_verified(process)
            };
            match outcome {
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
                    pending_identity_errors,
                    process.identity,
                    format!("pid {}: {err}", process.identity.pid),
                ),
            }
        }

        (killed, calls, None)
    }

    fn verify_cleanup_identity_errors(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
        pending_identity_errors: Vec<(ProcessIdentity, String)>,
        errors: &mut Vec<String>,
    ) {
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
    }

    fn complete_selection_facts(
        &self,
        selection: &mut Selection,
        roots: &[schema::ProcessIdentity],
    ) {
        let sample_id = Uuid::new_v4();
        for row in &mut selection.rows {
            if let Some(fresh) = row.fresh.as_mut() {
                fresh.sample_id = Some(sample_id);
            }
            let mut current = self.backend.observe_diagnostic_identity(row.expected.pid);
            if let Some(identity) = current.identity {
                current.status = if identity == row.expected {
                    schema::IdentityStatus::Same
                } else {
                    schema::IdentityStatus::Reused
                };
            }
            row.current = current;
            row.matches_app = self
                .app_identity
                .map(|id| selection::identity(id) == row.expected);
            row.matches_other_roots = roots
                .iter()
                .copied()
                .filter(|root| *root == row.expected)
                .collect();
        }
    }

    fn evidence_veto(
        &self,
        pass: &CleanupPass,
        operation_id: Uuid,
        error: schema::EvidenceError,
        killed_processes: Vec<ProcessIdentity>,
    ) -> CleanupFailure {
        let accounting = self
            .inner
            .lock()
            .map_err(|_error| "resource monitor lock poisoned".to_string())
            .and_then(|mut inner| {
                let group = inner
                    .groups
                    .get_mut(&pass.binding.session_id)
                    .ok_or_else(|| "resource cleanup registration missing".to_string())?;
                if !pass.owns(group) {
                    return Err("resource cleanup registration/transition changed".into());
                }
                let state = if error.effects_started {
                    group.state = ResourceGroupState::Quarantined;
                    "partial_quarantine".to_string()
                } else {
                    group.state = pass.previous_state;
                    group.kill_started_at = pass.previous_started;
                    format!("restored:{:?}", pass.previous_state)
                };
                group.last_error = Some(format!("evidence_gap: {error}; {state}"));
                Ok(state)
            });
        let (accounting_state, accounting_error) = match accounting {
            Ok(state) => (state, None),
            Err(error) => ("unknown".into(), Some(error)),
        };
        CleanupFailure::EvidenceVeto {
            result: ResourceKillResult {
                session_id: pass.binding.session_id.to_string(),
                state: ResourceGroupState::Quarantined,
                killed_processes,
                quarantined: true,
                message: format!(
                    "evidence_gap operation={operation_id}: {error}; {accounting_state}"
                ),
                blocked_by_security: false,
                finalized: false,
            },
            operation_id,
            stage: error.stage,
            effects_started: error.effects_started,
            accounting_state,
            accounting_error,
        }
    }

    // P05 supplies public/local session authorization before this registry-only read.
    #[allow(dead_code)]
    pub(crate) fn inspect_group(
        &self,
        session_id: Uuid,
    ) -> Result<schema::Receipt, schema::EvidenceError> {
        let (binding, root, retained, roots) = {
            let inner = self
                .inner
                .lock()
                .map_err(|error| schema::EvidenceError::new("inspect", error))?;
            let group = inner
                .groups
                .get(&session_id)
                .ok_or_else(|| schema::EvidenceError::new("inspect", "not_registered"))?;
            (
                schema::Binding {
                    run_id: self.diagnostics.run_id(),
                    session_id,
                    root: Some(selection::identity(group.root_identity)),
                    registration_generation: group.generation,
                },
                group.root_identity,
                group
                    .observed_processes
                    .values()
                    .cloned()
                    .collect::<Vec<_>>(),
                other_roots(&inner, session_id),
            )
        };
        let operation = self.diagnostics.inspect_operation(binding)?;
        let (fresh, errors) = match self.backend.observe_tree(root) {
            Ok(tree) => (tree.processes, tree.errors),
            Err(error) => (Vec::new(), vec![error.to_string()]),
        };
        let mut selection = selection::select_targets(&retained, &fresh);
        self.complete_selection_facts(&mut selection, &roots);
        let selected = operation.selection(&selection.rows);
        let finished = operation.finish(schema::Summary {
            partial: selected.is_err() || !errors.is_empty(),
            evidence_gap: selected.is_err(),
            killed_processes: 0,
            accounting: "proposed".into(),
            errors: errors.clone(),
        });
        selected?;
        finished?;
        Ok(schema::Receipt {
            status: "complete".into(),
            operation_id: Some(operation.id()),
            path: self.diagnostics.operation_path(operation.id()),
            evidence_gap: false,
            errors,
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

impl CleanupPass {
    fn owns(&self, group: &super::ResourceAgentGroup) -> bool {
        group.generation == self.binding.registration_generation
            && Some(selection::identity(group.root_identity)) == self.binding.root
            && group.state == ResourceGroupState::Terminating
            && group.kill_started_at == Some(self.started)
    }
}

fn other_roots(
    inner: &super::ResourceMonitorInner,
    session_id: Uuid,
) -> Vec<schema::ProcessIdentity> {
    inner
        .groups
        .values()
        .filter(|group| group.session_id != session_id)
        .map(|group| selection::identity(group.root_identity))
        .collect()
}
