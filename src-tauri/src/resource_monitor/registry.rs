use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;

use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
    ResourceAgentGroupSnapshot, ResourceGroupState, ResourceGroupWarning,
    ResourceGroupWarningLevel, ResourceKillReason, ResourceKillResult, ResourceLaunchMetadata,
    ResourceLimits, ResourceOverallState, ResourceProcessSnapshot, ResourceSnapshot,
    TerminateOutcome,
};

#[derive(Debug, Error, Clone)]
pub enum ResourceError {
    #[error("{0}")]
    Message(String),
}

impl From<String> for ResourceError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for ResourceError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}

pub trait ProcessTreeBackend: Send + Sync + 'static {
    fn supports_process_tree_enforcement(&self) -> bool {
        true
    }

    /// Reports the tree as the OS snapshot saw it. Identity resolution here is
    /// corpse-TOLERANT: an exited-but-still-queryable process keeps its identity
    /// (see #1438 G1); liveness filtering belongs to `observe_identity`.
    fn observe_tree(&self, root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError>;
    /// `Ok(Some(identity))` means a live process with that identity exists now. An
    /// exited process that is still queryable through open handles MUST yield
    /// `Ok(None)` (#1438).
    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError>;
    /// MUST return `Ok(Terminated)` when the target verifiably exited, including when
    /// other handles keep the corpse queryable. `Err` is reserved for a target that
    /// demonstrably survives or cannot be verified. Every `TerminateOutcome` variant a
    /// real backend can produce needs at least one real-process test producing it
    /// (#1438 Decision 3).
    fn terminate_verified(
        &self,
        process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError>;
    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError>;
}

#[derive(Clone)]
pub struct ResourceMonitorState {
    inner: Arc<Mutex<ResourceMonitorInner>>,
    backend: Arc<dyn ProcessTreeBackend>,
}

struct ResourceMonitorInner {
    groups: HashMap<Uuid, ResourceAgentGroup>,
    pending_permits: BTreeSet<u64>,
    next_generation: u64,
    last_snapshot: Option<ResourceSnapshot>,
}

struct ResourceAgentGroup {
    session_id: Uuid,
    name: String,
    agent_id: Option<String>,
    agent_label: Option<String>,
    workgroup: Option<String>,
    agent: Option<String>,
    project: Option<String>,
    root_identity: ProcessIdentity,
    state: ResourceGroupState,
    descendants_observed: bool,
    observed_processes: BTreeMap<ProcessIdentity, ObservedProcess>,
    kill_started_at: Option<Instant>,
    last_error: Option<String>,
    permit_released: bool,
    /// #559 - consecutive samples where observe_tree reported the root missing.
    /// Reaped only after two strikes so a single enumeration fluke never frees a
    /// live agent's slot. Reset to 0 whenever the root is observed alive or merely
    /// unreadable, so the counter is consecutive-only by design (a momentarily
    /// unreadable root means the pid still exists, i.e. possibly alive).
    missing_root_strikes: u8,
    /// #559 - when this group entered Terminated, used to bound retained
    /// terminated rows (prune oldest first) without unbounded map growth.
    terminated_at: Option<Instant>,
}

#[derive(Debug)]
pub struct AgentLaunchPermit {
    generation: u64,
}

/// #1151 - outcome of `retry_orphaned_quarantine`. `Skipped` performs ZERO backend
/// calls, so an ineligible group costs nothing and cannot be confused with a cleanup
/// that ran and found the group already gone.
#[derive(Debug, Clone)]
pub(crate) enum QuarantineRetryOutcome {
    Completed(ResourceKillResult),
    Skipped(QuarantineRetrySkip),
}

/// #1151 - why an orphan quarantine retry did not run. `NotRegistered` is defensive
/// only: a Quarantined row can never be removed from the group map, so a UUID sampled
/// as Quarantined in the same tick is always still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuarantineRetrySkip {
    Unsupported,
    NotRegistered,
    NotQuarantined(ResourceGroupState),
    RootMismatch,
}

impl QuarantineRetrySkip {
    pub(crate) fn as_reason(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NotRegistered => "notRegistered",
            Self::NotQuarantined(_) => "notQuarantined",
            Self::RootMismatch => "rootMismatch",
        }
    }
}

pub struct ResourceLaunchRegistration {
    monitor: ResourceMonitorState,
    permit: Option<AgentLaunchPermit>,
    metadata: ResourceLaunchMetadata,
    registered: bool,
}

pub struct ResourceLogicalAgentSlot {
    monitor: ResourceMonitorState,
    permit: Option<AgentLaunchPermit>,
}

impl Drop for ResourceLogicalAgentSlot {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.monitor.release_unregistered_permit(permit);
        }
    }
}

impl Drop for ResourceLaunchRegistration {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.monitor.release_unregistered_permit(permit);
        }
    }
}

impl ResourceLaunchRegistration {
    pub fn new(
        monitor: ResourceMonitorState,
        permit: AgentLaunchPermit,
        metadata: ResourceLaunchMetadata,
    ) -> Self {
        Self {
            monitor,
            permit: Some(permit),
            metadata,
            registered: false,
        }
    }

    pub fn register_root_pid(&mut self, pid: u32) -> Result<Option<ProcessIdentity>, String> {
        if !self.monitor.supports_process_tree_enforcement() {
            if let Some(permit) = self.permit.take() {
                self.monitor.release_unregistered_permit(permit);
            }
            return Ok(None);
        }
        let identity = self
            .monitor
            .observe_identity(pid)?
            .ok_or_else(|| format!("resource monitor could not observe spawned pid {pid}"))?;
        let permit = self
            .permit
            .take()
            .ok_or_else(|| "resource monitor launch permit already consumed".to_string())?;
        self.monitor.register_group(
            permit,
            self.metadata.session_id,
            self.metadata.name.clone(),
            self.metadata.agent_id.clone(),
            self.metadata.agent_label.clone(),
            self.metadata.workgroup.clone(),
            self.metadata.agent.clone(),
            self.metadata.project.clone(),
            identity,
        )?;
        self.registered = true;
        Ok(Some(identity))
    }

    pub fn rollback_registered(&self) {
        if self.registered {
            let _ = self
                .monitor
                .kill_group(self.metadata.session_id, ResourceKillReason::SpawnRollback);
        }
    }
}

impl ResourceMonitorState {
    pub fn new() -> Self {
        Self::with_backend(Arc::new(super::windows::PlatformProcessTreeBackend::new()))
    }

    pub fn with_backend(backend: Arc<dyn ProcessTreeBackend>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ResourceMonitorInner {
                groups: HashMap::new(),
                pending_permits: BTreeSet::new(),
                next_generation: 1,
                last_snapshot: None,
            })),
            backend,
        }
    }

    pub fn supports_process_tree_enforcement(&self) -> bool {
        self.backend.supports_process_tree_enforcement()
    }

    pub fn is_effectively_enabled(&self, configured_enabled: bool) -> bool {
        configured_enabled && self.supports_process_tree_enforcement()
    }

    pub fn try_reserve_agent_slot(
        &self,
        limits: ResourceLimits,
    ) -> Result<Option<AgentLaunchPermit>, String> {
        if !self.is_effectively_enabled(limits.monitor_enabled) {
            return Ok(None);
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "resource monitor lock poisoned")?;
        let active = active_count(&inner);
        if active >= limits.max_concurrent_agent_processes as usize {
            return Err(format!(
                "Resource Monitor cap reached: {active}/{} agent groups are active",
                limits.max_concurrent_agent_processes
            ));
        }
        let generation = inner.next_generation;
        inner.next_generation = inner.next_generation.saturating_add(1);
        inner.pending_permits.insert(generation);
        Ok(Some(AgentLaunchPermit { generation }))
    }

    pub fn release_unregistered_permit(&self, permit: AgentLaunchPermit) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending_permits.remove(&permit.generation);
        }
    }

    pub fn hold_logical_agent_slot(
        &self,
        permit: AgentLaunchPermit,
    ) -> Option<ResourceLogicalAgentSlot> {
        if !self.supports_process_tree_enforcement() {
            self.release_unregistered_permit(permit);
            return None;
        }
        Some(ResourceLogicalAgentSlot {
            monitor: self.clone(),
            permit: Some(permit),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn register_group(
        &self,
        permit: AgentLaunchPermit,
        session_id: Uuid,
        name: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        workgroup: Option<String>,
        agent: Option<String>,
        project: Option<String>,
        root_identity: ProcessIdentity,
    ) -> Result<(), String> {
        let observed = self.backend.observe_tree(root_identity);
        let mut observed_processes = BTreeMap::new();
        let last_error = match observed {
            Ok(tree) => {
                let last_error = join_errors(tree.errors);
                merge_observed(&mut observed_processes, tree.processes);
                last_error
            }
            Err(err) => Some(err.to_string()),
        };
        observed_processes
            .entry(root_identity)
            .or_insert_with(|| root_placeholder(root_identity));
        let descendants_observed = observed_processes.len() > 1;

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "resource monitor lock poisoned")?;
        if !inner.pending_permits.remove(&permit.generation) {
            return Err("resource monitor launch permit was not pending".to_string());
        }
        // #559 (H3) - a relaunch of the same wg/role must not leave its prior
        // Terminated row behind as a duplicate. Only dedup concrete identities so
        // unrelated non-WG launches (workgroup = agent = None) are never merged.
        if workgroup.is_some() || agent.is_some() {
            inner.groups.retain(|_, g| {
                !(matches!(g.state, ResourceGroupState::Terminated)
                    && g.workgroup == workgroup
                    && g.agent == agent
                    // #566 - also match project so two different projects running
                    // the same wg/role never dedup against each other's Terminated
                    // row. PREDICATE only; the gate above stays (wg|agent) so
                    // unrelated non-WG launches are never merged (preserves #559).
                    && g.project == project)
            });
        }
        let replaced = inner.groups.insert(
            session_id,
            ResourceAgentGroup {
                session_id,
                name,
                agent_id,
                agent_label,
                workgroup,
                agent,
                project,
                root_identity,
                state: ResourceGroupState::Running,
                descendants_observed,
                observed_processes,
                kill_started_at: None,
                last_error,
                permit_released: false,
                missing_root_strikes: 0,
                terminated_at: None,
            },
        );
        // #1151 - no flow reuses a session UUID today (relaunch and restart both allocate
        // a new one), so this insert should only ever replace a Terminated row the dedup
        // above chose to keep. If it ever replaces a live or Quarantined one, that row's
        // permit accounting is lost silently and the cap drifts. Diagnostic only:
        // an assert here would turn a bookkeeping drift into a panic on the launch path,
        // which is a strictly worse failure than the hazard it guards.
        if let Some(previous) = replaced {
            if !matches!(previous.state, ResourceGroupState::Terminated) {
                log::error!(
                    "[resource-monitor] register_group replaced a \
                        live row session={} previous_state={:?} previous_root_pid={} \
                        permit_released={}",
                    session_id,
                    previous.state,
                    previous.root_identity.pid,
                    previous.permit_released
                );
            }
        }
        Ok(())
    }

    pub fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, String> {
        self.backend
            .observe_identity(pid)
            .map_err(|e| e.to_string())
    }

    pub fn has_registered_group(&self, session_id: Uuid) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.groups.contains_key(&session_id))
            .unwrap_or(false)
    }

    pub fn active_agent_groups(&self) -> usize {
        if !self.supports_process_tree_enforcement() {
            return 0;
        }
        self.inner
            .lock()
            .map(|inner| active_count(&inner))
            .unwrap_or(0)
    }

    /// #559 (H2) - true if a quarantined group is eligible for another cleanup retry
    /// (its last cleanup attempt is older than the backoff). kill_started_at is
    /// refreshed by kill_group at the start of every attempt, so the backoff spaces
    /// attempts from the start of the previous one. A Quarantined group always has
    /// kill_started_at = Some (it can only reach Quarantined through kill_group), so
    /// the unwrap_or(true) branch is dead-but-defensive.
    pub fn quarantine_retry_due(&self, session_id: Uuid) -> bool {
        let Ok(inner) = self.inner.lock() else {
            return false;
        };
        match inner.groups.get(&session_id) {
            Some(group) if matches!(group.state, ResourceGroupState::Quarantined) => group
                .kill_started_at
                .map(|t| t.elapsed() >= QUARANTINE_RETRY_BACKOFF)
                .unwrap_or(true),
            _ => false,
        }
    }

    /// #1151 - on the real impl, not inside `mod tests`, so watchdog.rs's tests can reach
    /// it. Mirrors PtyManager::new_for_test (pty/manager.rs), which is `#[cfg(test)]
    /// pub(crate)` on the real impl for exactly this cross-module need.
    ///
    /// `checked_sub` rather than plain subtraction: `Instant`'s epoch is unspecified
    /// (machine boot on Windows), so `Instant::now() - QUARANTINE_RETRY_BACKOFF` can
    /// panic. The `expect` makes a failure loud rather than producing a test that
    /// silently is not due. Setting `kill_started_at = None` is deliberately NOT used:
    /// `quarantine_retry_due` calls that branch dead-but-defensive, and a test depending
    /// on it would convert a defensive branch into a load-bearing one.
    #[cfg(test)]
    pub(crate) fn test_backdate_quarantine_retry(&self, id: Uuid) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(group) = inner.groups.get_mut(&id) {
                let backdated = Instant::now()
                    .checked_sub(QUARANTINE_RETRY_BACKOFF + Duration::from_millis(1))
                    .expect("backdated quarantine retry instant is representable");
                group.kill_started_at = Some(backdated);
            }
        }
    }

    pub fn snapshot(&self, limits: ResourceLimits) -> ResourceSnapshot {
        if !self.is_effectively_enabled(limits.monitor_enabled) {
            return disabled_snapshot(limits);
        }
        let samples = self.sample_running_groups();
        let mut warnings = Vec::new();
        let mut reap_candidates: Vec<(Uuid, ProcessIdentity)> = Vec::new();
        for (session_id, result) in samples {
            match result {
                Ok(tree) => {
                    let outcome = self.merge_sample(session_id, tree);
                    if let Some(error) = outcome.warning {
                        warnings.push(error);
                    }
                    if let Some(root) = outcome.reap_candidate {
                        reap_candidates.push((session_id, root));
                    }
                }
                Err(err) => {
                    warnings.push(err.to_string());
                    self.set_group_error(session_id, err.to_string());
                }
            }
        }

        // #559 (H1) - confirm each reap candidate with a direct identity syscall
        // OUTSIDE the lock before the irreversible reap. observe_identity opens the
        // pid directly, so it does not depend on toolhelp-enumeration completeness
        // (closes G1) and is a fresh check at reap time (closes G2). Reap only when
        // the registered identity is confirmed gone (absent) or recycled (mismatch);
        // abort and reset strikes when the root is alive or unverifiable.
        for (session_id, root) in reap_candidates {
            match self.backend.observe_identity(root.pid) {
                Ok(Some(current)) if current == root => {
                    self.reset_missing_root_strikes(session_id, root);
                }
                Ok(_) => self.apply_reap(session_id, root),
                Err(_) => self.reset_missing_root_strikes(session_id, root),
            }
        }

        let app_memory = match self.backend.current_process_memory() {
            Ok(memory) => memory,
            Err(err) => {
                warnings.push(format!("app memory unavailable: {err}"));
                ProcessMemory::default()
            }
        };

        // #559 (H3) - bound retained Terminated rows after the reap, so a row reaped
        // this tick still renders as Terminated but the map cannot grow without limit.
        self.prune_terminated_groups();

        let snapshot = {
            let inner = self.inner.lock().expect("resource monitor lock poisoned");
            build_snapshot(&inner, limits, app_memory, warnings)
        };
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_snapshot = Some(snapshot.clone());
        }
        snapshot
    }

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
    fn kill_group_inner(
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

    pub fn sample_for_watchdog(
        &self,
        limits: ResourceLimits,
    ) -> Vec<(Uuid, ResourceAgentGroupSnapshot)> {
        let snapshot = self.snapshot(limits);
        snapshot
            .groups
            .into_iter()
            .filter_map(|group| {
                Uuid::parse_str(&group.session_id)
                    .ok()
                    .map(|id| (id, group))
            })
            .collect()
    }

    fn sample_running_groups(&self) -> Vec<(Uuid, Result<ObservedProcessTree, ResourceError>)> {
        let roots = self
            .inner
            .lock()
            .map(|inner| {
                inner
                    .groups
                    .iter()
                    .filter_map(|(id, group)| {
                        // #647 C: include Quarantined so its process list is
                        // re-observed each snapshot (unfreezes the display for the
                        // root-alive / EDR-residual case). Reap-safe: merge_sample
                        // only strike-counts/nominates Running groups.
                        matches!(
                            group.state,
                            ResourceGroupState::Running
                                | ResourceGroupState::Terminating
                                | ResourceGroupState::Quarantined
                        )
                        .then_some((*id, group.root_identity))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        roots
            .into_iter()
            .map(|(id, root)| (id, self.backend.observe_tree(root)))
            .collect()
    }

    fn merge_tree(&self, session_id: Uuid, tree: ObservedProcessTree) -> Option<String> {
        let mut inner = self.inner.lock().ok()?;
        let group = inner.groups.get_mut(&session_id)?;
        group.descendants_observed |= tree.processes.len() > 1;
        merge_observed(&mut group.observed_processes, tree.processes);
        let joined = join_errors(tree.errors);
        if joined.is_some() {
            group.last_error = joined.clone();
        }
        joined
    }

    /// #559 (H1) - snapshot-path merge. Advances the missing-root strike counter and,
    /// when it reaches REAP_STRIKES, NOMINATES the group for reap (returns its root
    /// identity) without reaping: the reap is gated by a direct observe_identity
    /// confirm done outside the lock in snapshot(). Preserves merge_tree's warning
    /// contract, except on the nominating sample, where it returns no warning so the
    /// raw missing-root string does not also flip overall_state to Warn (G4).
    fn merge_sample(&self, session_id: Uuid, tree: ObservedProcessTree) -> SampleOutcome {
        let none = SampleOutcome {
            warning: None,
            reap_candidate: None,
        };
        let Ok(mut inner) = self.inner.lock() else {
            return none;
        };
        let Some(group) = inner.groups.get_mut(&session_id) else {
            return none;
        };

        // #559 (H1) - read the signal from the structured errors BEFORE moving the tree.
        let root_missing = tree
            .errors
            .iter()
            .any(|e| is_missing_root_cleanup_error(e, group.root_identity));
        let root_alive = tree
            .processes
            .iter()
            .any(|p| p.identity == group.root_identity);

        // #632 B2b - capture the live identity set and whether the sample is CLEAN
        // before merge_observed consumes the tree. "Clean" = no enumeration errors at
        // all (no missing root, no per-pid "identity unavailable" placeholder), so a
        // partial/racy toolhelp snapshot can never drop a live descendant.
        let live_identities: BTreeSet<ProcessIdentity> =
            tree.processes.iter().map(|p| p.identity).collect();
        let sample_clean = tree.errors.is_empty();

        group.descendants_observed |= tree.processes.len() > 1;
        merge_observed(&mut group.observed_processes, tree.processes);

        // #632 B2b - on a CLEAN sample with the root observed alive, the fresh tree is
        // the authoritative live set, so drop entries no longer present. Gated on
        // `root_alive && sample_clean` (MED-3): a missing-root, partial, or
        // ACCESS_DENIED-placeholder sample keeps the last-known set - that case is
        // owned by the #559 strike/reap logic below, and keeping it avoids dropping a
        // live child a racy snapshot momentarily missed (which would weaken the no-job
        // reaper fallback). A clean tree always contains the root, so the root is
        // retained.
        //
        // KNOWN RESIDUAL (#637): a still-alive grandchild whose intermediate parent
        // exited is unreachable from the root in this BFS (Windows does not reparent)
        // yet the sample still looks clean, so it is pruned here. Harmless for jobbed
        // sessions (the Job Object kills the whole tree regardless of reachability);
        // for a job-less session it extends the MED-2 reaper-only orphan residual.
        if root_alive && sample_clean {
            group
                .observed_processes
                .retain(|identity, _| live_identities.contains(identity));
        }
        let joined = join_errors(tree.errors);
        if joined.is_some() {
            group.last_error = joined.clone();
        }

        if matches!(group.state, ResourceGroupState::Running) && root_missing && !root_alive {
            group.missing_root_strikes = group.missing_root_strikes.saturating_add(1);
            if group.missing_root_strikes >= REAP_STRIKES {
                // Nominate; suppress the raw warning on this sample (G4).
                return SampleOutcome {
                    warning: None,
                    reap_candidate: Some(group.root_identity),
                };
            }
        } else {
            group.missing_root_strikes = 0;
        }

        SampleOutcome {
            warning: joined,
            reap_candidate: None,
        }
    }

    /// #559 (H1) - perform the confirmed reap under the lock. Re-checks the group is
    /// still Running, still at the strike threshold, and still the same root, so a
    /// concurrent kill_group that already terminated it wins and this becomes a no-op.
    fn apply_reap(&self, session_id: Uuid, root: ProcessIdentity) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(group) = inner.groups.get_mut(&session_id) else {
            return;
        };
        if matches!(group.state, ResourceGroupState::Running)
            && group.missing_root_strikes >= REAP_STRIKES
            && group.root_identity == root
        {
            group.state = ResourceGroupState::Terminated;
            group.permit_released = true;
            group.terminated_at = Some(Instant::now());
            group.missing_root_strikes = 0;
            group.last_error = Some("agent process exited; resource slot reclaimed".to_string());
            log::info!(
                "[resource-monitor] reaped exited group session={} root_pid={}",
                session_id,
                root.pid
            );
        }
    }

    /// #559 (H1) - the confirm said the root is alive (or was unverifiable); clear the
    /// strikes so a future genuine exit starts counting fresh.
    fn reset_missing_root_strikes(&self, session_id: Uuid, root: ProcessIdentity) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(group) = inner.groups.get_mut(&session_id) {
                if matches!(group.state, ResourceGroupState::Running) && group.root_identity == root
                {
                    group.missing_root_strikes = 0;
                }
            }
        }
    }

    /// #559 (H3) - bound retained Terminated rows so inner.groups cannot grow without
    /// limit. Keeps the MAX_TERMINATED_RETAINED most recent; evicts the oldest.
    /// Released-permit Terminated groups never count toward active_count, so eviction
    /// never affects the cap.
    fn prune_terminated_groups(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let mut terminated: Vec<(Uuid, Instant)> = inner
            .groups
            .iter()
            .filter(|(_, g)| matches!(g.state, ResourceGroupState::Terminated) && g.permit_released)
            .map(|(id, g)| (*id, g.terminated_at.unwrap_or_else(Instant::now)))
            .collect();
        if terminated.len() <= MAX_TERMINATED_RETAINED {
            return;
        }
        terminated.sort_by_key(|(_, at)| *at); // oldest first
        let remove = terminated.len() - MAX_TERMINATED_RETAINED;
        for (id, _) in terminated.into_iter().take(remove) {
            inner.groups.remove(&id);
        }
    }

    fn set_group_error(&self, session_id: Uuid, error: String) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(group) = inner.groups.get_mut(&session_id) {
                group.last_error = Some(error);
            }
        }
    }
}

impl Default for ResourceMonitorState {
    fn default() -> Self {
        Self::new()
    }
}

fn active_count(inner: &ResourceMonitorInner) -> usize {
    inner.pending_permits.len()
        + inner
            .groups
            .values()
            .filter(|group| {
                !matches!(group.state, ResourceGroupState::Terminated) || !group.permit_released
            })
            .count()
}

fn merge_observed(
    target: &mut BTreeMap<ProcessIdentity, ObservedProcess>,
    processes: Vec<ObservedProcess>,
) {
    for process in processes {
        target.insert(process.identity, process);
    }
}

fn join_errors(errors: Vec<String>) -> Option<String> {
    (!errors.is_empty()).then(|| errors.join("; "))
}

/// #559 - result of a snapshot-path sample merge: the warning to surface (if any)
/// and, when the strike threshold is reached, the root identity nominated for reap.
struct SampleOutcome {
    warning: Option<String>,
    reap_candidate: Option<ProcessIdentity>,
}

/// #559 - consecutive missing-root samples required before a group is NOMINATED for
/// reap. A pre-filter only; the reap itself is gated by a direct observe_identity
/// confirm (see snapshot()), so this threshold never authorizes a reap on its own.
const REAP_STRIKES: u8 = 2;

/// #559 - upper bound on retained Terminated (released-permit) rows kept for
/// recent-history display. Deliberately a fixed bound, independent of the
/// active-slot cap `max_concurrent_agent_processes` (#565): eviction here never
/// affects active_count, so it does not need to track the configurable cap.
const MAX_TERMINATED_RETAINED: usize = 16;

/// #559 - minimum interval between cleanup retries for a single quarantine.
const QUARANTINE_RETRY_BACKOFF: Duration = Duration::from_secs(15);

/// #647 D: true if any error carries the EXACT ACCESS_DENIED code (`win32 error 5`),
/// so "win32 error 50/53/567" do NOT match. Errors are formatted "...win32 error {code}".
fn has_access_denied_signature(errors: &[String]) -> bool {
    errors.iter().any(|e| {
        e.split("win32 error ").skip(1).any(|tail| {
            tail.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                == "5"
        })
    })
}

fn is_missing_root_cleanup_error(error: &str, root_identity: ProcessIdentity) -> bool {
    error == format!("root pid {} was not in process snapshot", root_identity.pid)
}

fn identity_unavailable_pid(error: &str) -> Option<u32> {
    error
        .strip_prefix("identity unavailable for pid ")?
        .parse()
        .ok()
}

fn is_placeholder_identity(identity: ProcessIdentity) -> bool {
    identity.creation_time_100ns == 0
}

fn push_pending_identity_error(
    pending: &mut Vec<(ProcessIdentity, String)>,
    identity: ProcessIdentity,
    error: String,
) {
    if !pending
        .iter()
        .any(|(pending_identity, _)| *pending_identity == identity)
    {
        pending.push((identity, error));
    }
}

fn unverifiable_process_error(pid: u32) -> String {
    format!("pid {pid} identity unavailable; cleanup ownership could not be verified")
}

fn root_placeholder(identity: ProcessIdentity) -> ObservedProcess {
    ObservedProcess {
        identity,
        parent_pid: None,
        parent_identity: None,
        exe_name: format!("pid {}", identity.pid),
        depth: 0,
        private_bytes: None,
        working_set_bytes: None,
        cpu_percent: None,
        kill_allowed: true,
    }
}

fn build_snapshot(
    inner: &ResourceMonitorInner,
    limits: ResourceLimits,
    app_memory: ProcessMemory,
    warnings: Vec<String>,
) -> ResourceSnapshot {
    let groups = inner
        .groups
        .values()
        .map(group_snapshot)
        .collect::<Vec<_>>();
    let mut group_warnings = Vec::new();
    for group in &groups {
        if let Some(bytes) = group.private_bytes {
            let level = if bytes >= limits.group_kill_private_bytes {
                Some((
                    ResourceGroupWarningLevel::Kill,
                    limits.group_kill_private_bytes,
                ))
            } else if bytes >= limits.group_warn_private_bytes {
                Some((
                    ResourceGroupWarningLevel::Warn,
                    limits.group_warn_private_bytes,
                ))
            } else {
                None
            };
            if let Some((level, limit_bytes)) = level {
                group_warnings.push(ResourceGroupWarning {
                    session_id: group.session_id.clone(),
                    level,
                    private_bytes: bytes,
                    limit_bytes,
                });
            }
        }
    }
    let active = active_count(inner);
    let has_quarantine = groups
        .iter()
        .any(|group| group.state == ResourceGroupState::Quarantined);
    let has_terminating = groups
        .iter()
        .any(|group| group.state == ResourceGroupState::Terminating);
    let has_critical = groups.iter().any(|group| {
        group
            .private_bytes
            .is_some_and(|bytes| bytes >= limits.group_kill_private_bytes)
            || group.processes.iter().any(|process| {
                process
                    .private_bytes
                    .is_some_and(|bytes| bytes >= limits.process_kill_private_bytes)
            })
    });
    let overall_state = if has_quarantine
        || has_terminating
        || active >= limits.max_concurrent_agent_processes as usize
    {
        ResourceOverallState::Enforcing
    } else if has_critical {
        ResourceOverallState::Critical
    } else if !warnings.is_empty() || !group_warnings.is_empty() {
        ResourceOverallState::Warn
    } else {
        ResourceOverallState::Ok
    };

    ResourceSnapshot {
        captured_at: Utc::now(),
        overall_state,
        monitor_enabled: limits.monitor_enabled,
        active_agent_groups: active,
        max_concurrent_agent_groups: limits.max_concurrent_agent_processes,
        app_private_bytes: app_memory.private_bytes,
        app_working_set_bytes: app_memory.working_set_bytes,
        groups,
        warnings,
        group_warnings,
    }
}

fn group_snapshot(group: &ResourceAgentGroup) -> ResourceAgentGroupSnapshot {
    let mut processes = group
        .observed_processes
        .values()
        .map(ResourceProcessSnapshot::from)
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| (process.name.clone(), process.pid));
    let private_bytes = sum_optional(processes.iter().map(|p| p.private_bytes));
    let working_set_bytes = sum_optional(processes.iter().map(|p| p.working_set_bytes));
    ResourceAgentGroupSnapshot {
        session_id: group.session_id.to_string(),
        name: group
            .agent_label
            .clone()
            .or_else(|| group.agent_id.clone())
            .unwrap_or_else(|| group.name.clone()),
        workgroup: group.workgroup.clone(),
        agent: group.agent.clone(),
        project: group.project.clone(),
        root_pid: group.root_identity.pid,
        root_identity: group.root_identity,
        state: group.state,
        descendants_observed: group.descendants_observed,
        process_count: processes.len(),
        private_bytes,
        working_set_bytes,
        cpu_percent: None,
        processes,
        kill_allowed: matches!(
            group.state,
            ResourceGroupState::Running | ResourceGroupState::Terminating
        ),
        last_error: group.last_error.clone(),
    }
}

fn disabled_snapshot(limits: ResourceLimits) -> ResourceSnapshot {
    ResourceSnapshot {
        captured_at: Utc::now(),
        overall_state: ResourceOverallState::Unknown,
        monitor_enabled: false,
        active_agent_groups: 0,
        max_concurrent_agent_groups: limits.max_concurrent_agent_processes,
        app_private_bytes: None,
        app_working_set_bytes: None,
        groups: Vec::new(),
        warnings: Vec::new(),
        group_warnings: Vec::new(),
    }
}

fn sum_optional(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut any = false;
    let mut total = 0_u64;
    for value in values.flatten() {
        any = true;
        total = total.saturating_add(value);
    }
    any.then_some(total)
}

#[cfg(test)]
mod tests;
