use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;

use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
    ResourceAgentGroupSnapshot, ResourceGroupState, ResourceKillReason, ResourceKillResult,
    ResourceLaunchMetadata, ResourceLimits, ResourceNetworkState, ResourceOverallState,
    ResourceProcessSnapshot, ResourceSnapshot, TerminateOutcome,
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
    fn observe_tree(&self, root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError>;
    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError>;
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

pub struct ResourceLaunchRegistration {
    monitor: ResourceMonitorState,
    permit: Option<AgentLaunchPermit>,
    metadata: ResourceLaunchMetadata,
    registered: bool,
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

    pub fn register_root_pid(&mut self, pid: u32) -> Result<ProcessIdentity, String> {
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
        Ok(identity)
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

    pub fn try_reserve_agent_slot(
        &self,
        limits: ResourceLimits,
    ) -> Result<Option<AgentLaunchPermit>, String> {
        if !limits.monitor_enabled {
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
        inner.groups.insert(
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

    pub fn snapshot(&self, limits: ResourceLimits) -> ResourceSnapshot {
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

        let mut snapshot = {
            let inner = self.inner.lock().expect("resource monitor lock poisoned");
            build_snapshot(&inner, limits, app_memory, warnings)
        };
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_snapshot = Some(snapshot.clone());
        }
        if snapshot.network_state == ResourceNetworkState::Unknown
            && matches!(snapshot.overall_state, ResourceOverallState::Ok)
        {
            snapshot.overall_state = ResourceOverallState::Unknown;
        }
        snapshot
    }

    pub fn kill_group(
        &self,
        session_id: Uuid,
        reason: ResourceKillReason,
    ) -> Result<ResourceKillResult, String> {
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
                });
            };
            match group.state {
                ResourceGroupState::Terminating => {
                    return Ok(ResourceKillResult {
                        session_id: session_id.to_string(),
                        state: ResourceGroupState::Terminating,
                        killed_processes: Vec::new(),
                        quarantined: false,
                        message: "resource group is already terminating".to_string(),
                    });
                }
                ResourceGroupState::Terminated => {
                    return Ok(ResourceKillResult {
                        session_id: session_id.to_string(),
                        state: ResourceGroupState::Terminated,
                        killed_processes: Vec::new(),
                        quarantined: false,
                        message: "resource group already terminated".to_string(),
                    });
                }
                ResourceGroupState::Running | ResourceGroupState::Quarantined => {}
            }
            group.state = ResourceGroupState::Terminating;
            group.kill_started_at = Some(Instant::now());
            (
                group.root_identity,
                group
                    .observed_processes
                    .values()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };

        let mut targets = previous_targets;
        let mut current_cleanup_identities = BTreeSet::new();
        let mut pending_identity_errors = Vec::new();
        let mut errors = Vec::new();
        match self.backend.observe_tree(root_identity) {
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
                        "[resource-monitor] cleanup sample for session {} had ignored root-missing error: {}",
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
                        "[resource-monitor] ignored cleanup identity error for gone pid={} session={}",
                        identity.pid,
                        session_id
                    );
                }
                Ok(Some(current)) if !is_placeholder_identity(identity) && current != identity => {
                    log::debug!(
                        "[resource-monitor] ignored cleanup identity error for reused pid={} session={}",
                        identity.pid,
                        session_id
                    );
                }
                Ok(Some(_)) | Err(_) => errors.push(error),
            }
        }

        let quarantined = !errors.is_empty();
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

        Ok(ResourceKillResult {
            session_id: session_id.to_string(),
            state,
            killed_processes: killed,
            quarantined,
            message,
        })
    }

    pub fn kill_all_owned_groups(&self, reason: ResourceKillReason) -> Vec<ResourceKillResult> {
        let ids = self
            .inner
            .lock()
            .map(|inner| inner.groups.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        ids.into_iter()
            .filter_map(|id| self.kill_group(id, reason).ok())
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
                        matches!(
                            group.state,
                            ResourceGroupState::Running | ResourceGroupState::Terminating
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

        group.descendants_observed |= tree.processes.len() > 1;
        merge_observed(&mut group.observed_processes, tree.processes);
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
                if matches!(group.state, ResourceGroupState::Running)
                    && group.root_identity == root
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
            .filter(|(_, g)| {
                matches!(g.state, ResourceGroupState::Terminated) && g.permit_released
            })
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
    mut warnings: Vec<String>,
) -> ResourceSnapshot {
    let groups = inner
        .groups
        .values()
        .map(group_snapshot)
        .collect::<Vec<_>>();
    for group in &groups {
        if let Some(bytes) = group.private_bytes {
            if bytes >= limits.group_kill_private_bytes {
                warnings.push(format!("{} exceeds group kill threshold", group.name));
            } else if bytes >= limits.group_warn_private_bytes {
                warnings.push(format!("{} exceeds group warn threshold", group.name));
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
    } else if !warnings.is_empty() {
        ResourceOverallState::Warn
    } else {
        ResourceOverallState::Unknown
    };

    ResourceSnapshot {
        captured_at: Utc::now(),
        overall_state,
        monitor_enabled: limits.monitor_enabled,
        active_agent_groups: active,
        max_concurrent_agent_groups: limits.max_concurrent_agent_processes,
        app_private_bytes: app_memory.private_bytes,
        app_working_set_bytes: app_memory.working_set_bytes,
        network_state: ResourceNetworkState::Unknown,
        network_summary: "Socket attribution unavailable".to_string(),
        groups,
        warnings,
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
        network_state: ResourceNetworkState::Unknown,
        network_summary: "Socket attribution unavailable".to_string(),
        processes,
        kill_allowed: matches!(
            group.state,
            ResourceGroupState::Running | ResourceGroupState::Terminating
        ),
        last_error: group.last_error.clone(),
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
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct FakeProcessTreeBackend {
        trees: Mutex<HashMap<ProcessIdentity, ObservedProcessTree>>,
        identities: Mutex<HashMap<u32, ProcessIdentity>>,
        stale: Mutex<BTreeSet<ProcessIdentity>>,
        stubborn: Mutex<BTreeSet<ProcessIdentity>>,
        unverifiable: Mutex<BTreeSet<u32>>,
        exit_during_terminate: Mutex<BTreeSet<ProcessIdentity>>,
        remove_on_terminate: Mutex<HashMap<ProcessIdentity, Vec<ProcessIdentity>>>,
        terminated: Mutex<Vec<ProcessIdentity>>,
        /// #559 (T1) - roots flagged here make observe_tree return Err, modelling a
        /// failed enumeration. The Err arm of snapshot() must never reach the strike
        /// counter or reap.
        fail_observe: Mutex<BTreeSet<ProcessIdentity>>,
    }

    impl FakeProcessTreeBackend {
        fn add_tree(&self, root: ProcessIdentity, processes: Vec<ObservedProcess>) {
            self.identities.lock().unwrap().insert(root.pid, root);
            for process in &processes {
                self.identities
                    .lock()
                    .unwrap()
                    .insert(process.identity.pid, process.identity);
            }
            self.trees.lock().unwrap().insert(
                root,
                ObservedProcessTree {
                    processes,
                    errors: Vec::new(),
                },
            );
        }

        fn replace_tree(
            &self,
            root: ProcessIdentity,
            processes: Vec<ObservedProcess>,
            errors: Vec<String>,
        ) {
            self.trees
                .lock()
                .unwrap()
                .insert(root, ObservedProcessTree { processes, errors });
        }

        fn mark_stale(&self, identity: ProcessIdentity) {
            self.stale.lock().unwrap().insert(identity);
            self.identities.lock().unwrap().insert(
                identity.pid,
                ProcessIdentity {
                    pid: identity.pid,
                    creation_time_100ns: identity.creation_time_100ns.saturating_add(1),
                },
            );
        }

        fn mark_gone(&self, identity: ProcessIdentity) {
            self.identities.lock().unwrap().remove(&identity.pid);
        }

        fn mark_present(&self, identity: ProcessIdentity) {
            self.identities
                .lock()
                .unwrap()
                .insert(identity.pid, identity);
        }

        fn mark_stubborn(&self, identity: ProcessIdentity) {
            self.stubborn.lock().unwrap().insert(identity);
        }

        fn mark_unverifiable(&self, pid: u32) {
            self.unverifiable.lock().unwrap().insert(pid);
        }

        fn mark_fail_observe(&self, identity: ProcessIdentity) {
            self.fail_observe.lock().unwrap().insert(identity);
        }

        fn mark_exit_during_terminate(&self, identity: ProcessIdentity) {
            self.exit_during_terminate.lock().unwrap().insert(identity);
        }

        fn mark_remove_on_terminate(
            &self,
            trigger: ProcessIdentity,
            removed: Vec<ProcessIdentity>,
        ) {
            self.remove_on_terminate
                .lock()
                .unwrap()
                .insert(trigger, removed);
        }

        fn terminated(&self) -> Vec<ProcessIdentity> {
            self.terminated.lock().unwrap().clone()
        }
    }

    impl ProcessTreeBackend for FakeProcessTreeBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            if self.fail_observe.lock().unwrap().contains(&root) {
                return Err(ResourceError::Message("observe failed".into()));
            }
            Ok(self
                .trees
                .lock()
                .unwrap()
                .get(&root)
                .cloned()
                .unwrap_or_default())
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            let current = self.identities.lock().unwrap().get(&pid).copied();
            if current.is_none() {
                return Ok(None);
            }
            if self.unverifiable.lock().unwrap().contains(&pid) {
                return Err(ResourceError::Message(format!(
                    "identity unavailable for pid {pid}"
                )));
            }
            Ok(current)
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            if self.stale.lock().unwrap().contains(&process.identity) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            if self
                .unverifiable
                .lock()
                .unwrap()
                .contains(&process.identity.pid)
            {
                return Err(ResourceError::Message(format!(
                    "identity unavailable for pid {}",
                    process.identity.pid
                )));
            }
            let current = self
                .identities
                .lock()
                .unwrap()
                .get(&process.identity.pid)
                .copied();
            if current != Some(process.identity) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            if self
                .exit_during_terminate
                .lock()
                .unwrap()
                .contains(&process.identity)
            {
                self.identities
                    .lock()
                    .unwrap()
                    .remove(&process.identity.pid);
                return Ok(TerminateOutcome::AlreadyGone);
            }
            if self.stubborn.lock().unwrap().contains(&process.identity) {
                return Err(ResourceError::Message(
                    "process still alive after terminate".to_string(),
                ));
            }
            self.terminated.lock().unwrap().push(process.identity);
            let removed_after_terminate = self
                .remove_on_terminate
                .lock()
                .unwrap()
                .get(&process.identity)
                .cloned()
                .unwrap_or_default();
            {
                let mut identities = self.identities.lock().unwrap();
                identities.remove(&process.identity.pid);
                for identity in removed_after_terminate {
                    identities.remove(&identity.pid);
                }
            }
            Ok(TerminateOutcome::Terminated)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    fn identity(pid: u32, tick: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            creation_time_100ns: tick,
        }
    }

    fn observed(pid: u32, tick: u64, parent_pid: Option<u32>, depth: u32) -> ObservedProcess {
        ObservedProcess {
            identity: identity(pid, tick),
            parent_pid,
            parent_identity: parent_pid.map(|p| identity(p, p as u64)),
            exe_name: format!("p{pid}"),
            depth,
            private_bytes: Some(pid as u64),
            working_set_bytes: Some(pid as u64),
            cpu_percent: None,
            kill_allowed: true,
        }
    }

    fn observed_unverifiable(pid: u32, parent_pid: Option<u32>, depth: u32) -> ObservedProcess {
        ObservedProcess {
            identity: ProcessIdentity {
                pid,
                creation_time_100ns: 0,
            },
            parent_pid,
            parent_identity: parent_pid.map(|p| identity(p, p as u64)),
            exe_name: format!("p{pid}"),
            depth,
            private_bytes: Some(pid as u64),
            working_set_bytes: Some(pid as u64),
            cpu_percent: None,
            kill_allowed: false,
        }
    }

    fn state_with_fake() -> (ResourceMonitorState, Arc<FakeProcessTreeBackend>) {
        let backend = Arc::new(FakeProcessTreeBackend::default());
        (
            ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>),
            backend,
        )
    }

    fn limits(max: u32) -> ResourceLimits {
        ResourceLimits {
            monitor_enabled: true,
            max_concurrent_agent_processes: max,
            group_warn_private_bytes: 100,
            group_kill_private_bytes: 200,
            process_kill_private_bytes: 200,
        }
    }

    #[test]
    fn cap_rejects_when_active_plus_pending_reaches_limit() {
        let (state, _backend) = state_with_fake();
        let _permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let err = state.try_reserve_agent_slot(limits(1)).unwrap_err();
        assert!(err.contains("cap reached"));
    }

    #[test]
    fn snapshot_merges_descendants() {
        let (state, backend) = state_with_fake();
        let root = identity(10, 10);
        backend.add_tree(
            root,
            vec![observed(10, 10, None, 0), observed(11, 11, Some(10), 1)],
        );
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        state
            .register_group(permit, Uuid::new_v4(), "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let snapshot = state.snapshot(limits(3));
        assert_eq!(snapshot.groups[0].process_count, 2);
        assert!(snapshot.groups[0].descendants_observed);
        assert_eq!(snapshot.network_state, ResourceNetworkState::Unknown);
        assert_eq!(snapshot.overall_state, ResourceOverallState::Unknown);
    }

    #[test]
    fn kill_uses_deepest_first_order() {
        let (state, backend) = state_with_fake();
        let root = identity(20, 20);
        backend.add_tree(
            root,
            vec![
                observed(20, 20, None, 0),
                observed(21, 21, Some(20), 1),
                observed(22, 22, Some(21), 2),
            ],
        );
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(!result.quarantined);
        let killed = backend.terminated();
        assert_eq!(killed[0].pid, 22);
        assert_eq!(killed[1].pid, 21);
        assert_eq!(killed[2].pid, 20);
        assert_eq!(state.active_agent_groups(), 0);
    }

    #[test]
    fn stale_identity_is_not_killed_and_releases_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(30, 30);
        let child = identity(31, 31);
        backend.add_tree(
            root,
            vec![observed(30, 30, None, 0), observed(31, 31, Some(30), 1)],
        );
        backend.mark_stale(child);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert!(!backend.terminated().contains(&child));
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn stubborn_identity_quarantines_and_keeps_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(32, 32);
        let child = identity(33, 33);
        backend.add_tree(
            root,
            vec![observed(32, 32, None, 0), observed(33, 33, Some(32), 1)],
        );
        backend.mark_stubborn(child);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Quarantined);
        assert_eq!(state.active_agent_groups(), 1);
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    #[test]
    fn vanished_root_and_already_gone_identities_release_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(34, 34);
        backend.add_tree(root, vec![observed(34, 34, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.mark_gone(root);
        backend.replace_tree(
            root,
            Vec::new(),
            vec![format!("root pid {} was not in process snapshot", root.pid)],
        );

        let result = state
            .kill_group(id, ResourceKillReason::SessionDestroy)
            .unwrap();
        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert!(result.killed_processes.is_empty());
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn observed_unverifiable_process_quarantines_and_keeps_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(35, 35);
        let unverifiable_child_pid = 36;
        let unverifiable_child = ProcessIdentity {
            pid: unverifiable_child_pid,
            creation_time_100ns: 0,
        };
        backend.add_tree(root, vec![observed(35, 35, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![
                observed(35, 35, None, 0),
                observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
            ],
            vec![format!(
                "identity unavailable for pid {unverifiable_child_pid}"
            )],
        );
        backend.mark_present(unverifiable_child);
        backend.mark_unverifiable(unverifiable_child.pid);

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(result.quarantined);
        assert!(result.message.contains("identity unavailable"));
        assert_eq!(state.active_agent_groups(), 1);
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    #[test]
    fn child_identity_error_clears_if_parent_termination_removes_child() {
        let (state, backend) = state_with_fake();
        let root = identity(41, 41);
        let child = identity(42, 42);
        backend.add_tree(
            root,
            vec![observed(41, 41, None, 0), observed(42, 42, Some(41), 1)],
        );
        backend.mark_unverifiable(child.pid);
        backend.mark_remove_on_terminate(root, vec![child]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();

        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert_eq!(result.killed_processes, vec![root]);
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn cleanup_sample_child_identity_error_clears_if_parent_termination_removes_child() {
        let (state, backend) = state_with_fake();
        let root = identity(43, 43);
        let unverifiable_child_pid = 44;
        let unverifiable_child = ProcessIdentity {
            pid: unverifiable_child_pid,
            creation_time_100ns: 0,
        };
        backend.add_tree(root, vec![observed(43, 43, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![
                observed(43, 43, None, 0),
                observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
            ],
            vec![format!(
                "identity unavailable for pid {unverifiable_child_pid}"
            )],
        );
        backend.mark_present(unverifiable_child);
        backend.mark_unverifiable(unverifiable_child.pid);
        backend.mark_remove_on_terminate(root, vec![unverifiable_child]);

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();

        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert_eq!(result.killed_processes, vec![root]);
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn cleanup_sample_identity_error_for_prior_target_clears_if_kill_removes_all_pids() {
        let (state, backend) = state_with_fake();
        let root = identity(49, 49);
        let node = identity(50, 50);
        let codex = identity(51, 51);
        backend.add_tree(
            root,
            vec![
                observed(49, 49, None, 0),
                observed(50, 50, Some(root.pid), 1),
                observed(51, 51, Some(node.pid), 2),
            ],
        );
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![observed(49, 49, None, 0)],
            vec![format!("identity unavailable for pid {}", node.pid)],
        );

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();

        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert_eq!(backend.terminated(), vec![codex, node, root]);
        assert!(state.observe_identity(root.pid).unwrap().is_none());
        assert!(state.observe_identity(node.pid).unwrap().is_none());
        assert!(state.observe_identity(codex.pid).unwrap().is_none());
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn cleanup_sample_placeholder_identity_error_quarantines_when_child_still_alive() {
        let (state, backend) = state_with_fake();
        let root = identity(45, 45);
        let child_pid = 46;
        let concrete_child = identity(child_pid, 46);
        backend.add_tree(root, vec![observed(45, 45, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![
                observed(45, 45, None, 0),
                observed_unverifiable(child_pid, Some(root.pid), 1),
            ],
            vec![format!("identity unavailable for pid {child_pid}")],
        );
        backend.mark_present(concrete_child);

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();

        assert!(result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Quarantined);
        assert!(result.message.contains("identity unavailable"));
        assert_eq!(backend.terminated(), vec![root]);
        assert_eq!(state.active_agent_groups(), 1);
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    #[test]
    fn stale_unverifiable_placeholder_gone_before_cleanup_releases_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(36, 36);
        let unverifiable_child_pid = 360;
        backend.add_tree(root, vec![observed(36, 36, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![
                observed(36, 36, None, 0),
                observed_unverifiable(unverifiable_child_pid, Some(root.pid), 1),
            ],
            vec![format!(
                "identity unavailable for pid {unverifiable_child_pid}"
            )],
        );
        let snapshot = state.snapshot(limits(1));
        assert_eq!(snapshot.groups[0].process_count, 2);

        backend.replace_tree(root, vec![observed(36, 36, None, 0)], Vec::new());

        let result = state
            .kill_group(id, ResourceKillReason::SessionDestroy)
            .unwrap();
        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert_eq!(backend.terminated(), vec![root]);
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn stale_unverifiable_placeholder_still_alive_quarantines() {
        let (state, backend) = state_with_fake();
        let root = identity(47, 47);
        let child_pid = 48;
        let concrete_child = identity(child_pid, 48);
        backend.add_tree(root, vec![observed(47, 47, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(
            root,
            vec![
                observed(47, 47, None, 0),
                observed_unverifiable(child_pid, Some(root.pid), 1),
            ],
            vec![format!("identity unavailable for pid {child_pid}")],
        );
        let snapshot = state.snapshot(limits(1));
        assert_eq!(snapshot.groups[0].process_count, 2);

        backend.replace_tree(root, vec![observed(47, 47, None, 0)], Vec::new());
        backend.mark_present(concrete_child);

        let result = state
            .kill_group(id, ResourceKillReason::SessionDestroy)
            .unwrap();

        assert!(result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Quarantined);
        assert!(result.message.contains("identity unavailable"));
        assert_eq!(backend.terminated(), vec![root]);
        assert_eq!(state.active_agent_groups(), 1);
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    #[test]
    fn observe_identity_error_quarantines_and_keeps_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(37, 37);
        backend.add_tree(root, vec![observed(37, 37, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.mark_unverifiable(root.pid);
        backend.replace_tree(
            root,
            Vec::new(),
            vec![format!("root pid {} was not in process snapshot", root.pid)],
        );

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(result.quarantined);
        assert!(result.message.contains("identity unavailable"));
        assert_eq!(state.active_agent_groups(), 1);
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    #[test]
    fn exited_during_termination_releases_permit() {
        let (state, backend) = state_with_fake();
        let root = identity(38, 38);
        backend.add_tree(root, vec![observed(38, 38, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.mark_exit_during_terminate(root);

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(!result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Terminated);
        assert!(result.killed_processes.is_empty());
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    #[test]
    fn double_kill_is_idempotent_after_termination() {
        let (state, backend) = state_with_fake();
        let root = identity(40, 40);
        backend.add_tree(root, vec![observed(40, 40, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let first = state.kill_group(id, ResourceKillReason::User).unwrap();
        let second = state.kill_group(id, ResourceKillReason::Watchdog).unwrap();
        assert_eq!(first.state, ResourceGroupState::Terminated);
        assert_eq!(second.state, ResourceGroupState::Terminated);
        assert_eq!(backend.terminated().len(), 1);
    }

    #[test]
    fn disabled_monitor_does_not_reserve_new_slot() {
        let (state, _backend) = state_with_fake();
        let mut disabled = limits(1);
        disabled.monitor_enabled = false;
        assert!(state.try_reserve_agent_slot(disabled).unwrap().is_none());
        assert_eq!(state.active_agent_groups(), 0);
    }

    #[test]
    fn dynamic_cap_shrink_rejects_new_launches() {
        let (state, _backend) = state_with_fake();
        let _a = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
        let _b = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
        assert!(state.try_reserve_agent_slot(limits(1)).is_err());
    }

    // #559 - test-only accessors into the internal group state. `mod tests` is a child
    // of the registry module, so it can read the private `inner` field directly.
    impl ResourceMonitorState {
        fn test_strikes(&self, id: Uuid) -> Option<u8> {
            self.inner
                .lock()
                .unwrap()
                .groups
                .get(&id)
                .map(|g| g.missing_root_strikes)
        }

        fn test_state(&self, id: Uuid) -> Option<ResourceGroupState> {
            self.inner.lock().unwrap().groups.get(&id).map(|g| g.state)
        }
    }

    fn missing_root_error(root: ProcessIdentity) -> Vec<String> {
        vec![format!(
            "root pid {} was not in process snapshot",
            root.pid
        )]
    }

    // (a) H1 - a naturally-exited root reaps after two strikes + a confirmed-gone check.
    #[test]
    fn natural_exit_reaps_after_two_strikes_and_confirm() {
        let (state, backend) = state_with_fake();
        let root = identity(60, 60);
        backend.add_tree(root, vec![observed(60, 60, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // Live root: no strike.
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.active_agent_groups(), 1);

        // Root exits: tree reports it missing, observe_identity confirms gone.
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        backend.mark_gone(root);

        // First strike: still Running (the two-strike pre-filter).
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.test_strikes(id), Some(1));
        assert_eq!(state.active_agent_groups(), 1);

        // Second strike + confirm gone: reaped, slot reclaimed.
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    // (a2) H1 - a live sample between two missing-root samples resets the strike count.
    #[test]
    fn missing_root_strike_resets_on_live_sample() {
        let (state, backend) = state_with_fake();
        let root = identity(62, 62);
        backend.add_tree(root, vec![observed(62, 62, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // One missing-root strike.
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        state.snapshot(limits(1));
        assert_eq!(state.test_strikes(id), Some(1));

        // Live again: strike resets.
        backend.replace_tree(root, vec![observed(62, 62, None, 0)], Vec::new());
        state.snapshot(limits(1));
        assert_eq!(state.test_strikes(id), Some(0));

        // A single fresh missing-root sample only reaches one strike again: no reap.
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        state.snapshot(limits(1));
        assert_eq!(state.test_strikes(id), Some(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.active_agent_groups(), 1);
    }

    // (a3 = T1, MANDATORY) H1 - observe_tree Err must never reap or touch the strikes.
    #[test]
    fn never_reaps_when_observe_tree_errors() {
        let (state, backend) = state_with_fake();
        let root = identity(63, 63);
        backend.add_tree(root, vec![observed(63, 63, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // A failed enumeration routes through set_group_error (the Err arm), which never
        // reaches merge_sample. Assert no reap and no strike change across two ticks.
        backend.mark_fail_observe(root);
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.test_strikes(id), Some(0));
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.test_strikes(id), Some(0));
        assert_eq!(state.active_agent_groups(), 1);
    }

    // (a4) H1 - a recycled root (PID reuse) reaps after two strikes + a mismatch confirm.
    #[test]
    fn recycled_root_reaps_after_two_strikes() {
        let (state, backend) = state_with_fake();
        let root = identity(64, 64);
        backend.add_tree(root, vec![observed(64, 64, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // Tree reports missing-root (the creation-time-mismatch path) and the confirm
        // returns a bumped identity (foreign process recycled the pid).
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        backend.mark_stale(root);

        state.snapshot(limits(1));
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    // (a5) H1 - a spurious missing-root signal on two ticks is overridden by the confirm
    // when the root is actually still alive: no reap, strikes reset.
    #[test]
    fn confirm_overrides_spurious_missing_root_signal() {
        let (state, backend) = state_with_fake();
        let root = identity(65, 65);
        backend.add_tree(root, vec![observed(65, 65, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // Root stays in the identities map, so observe_identity returns Ok(Some(root)).
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        state.snapshot(limits(1));
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.test_strikes(id), Some(0));
        assert_eq!(state.active_agent_groups(), 1);
    }

    // (a6) H1 - an unverifiable confirm (observe_identity Err) aborts the reap.
    #[test]
    fn unverifiable_confirm_aborts_reap() {
        let (state, backend) = state_with_fake();
        let root = identity(66, 66);
        backend.add_tree(root, vec![observed(66, 66, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        // Missing-root signal fires, but observe_identity returns Err (pid still exists,
        // momentarily unreadable): conservative abort.
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        backend.mark_unverifiable(root.pid);
        state.snapshot(limits(1));
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Running));
        assert_eq!(state.test_strikes(id), Some(0));
        assert_eq!(state.active_agent_groups(), 1);
    }

    // (b) H2 - re-invoking kill_group on a quarantine after the blocker clears frees it.
    #[test]
    fn quarantine_recleanup_frees_slot() {
        let (state, backend) = state_with_fake();
        let root = identity(67, 67);
        let child = identity(68, 68);
        backend.add_tree(
            root,
            vec![observed(67, 67, None, 0), observed(68, 68, Some(67), 1)],
        );
        backend.mark_stubborn(child);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(result.quarantined);
        assert_eq!(result.state, ResourceGroupState::Quarantined);
        assert_eq!(state.active_agent_groups(), 1);

        // The stubborn child finally exits; the watchdog-driven retry now cleans up.
        backend.mark_gone(child);
        let retry = state.kill_group(id, ResourceKillReason::Watchdog).unwrap();
        assert!(!retry.quarantined);
        assert_eq!(retry.state, ResourceGroupState::Terminated);
        assert_eq!(state.active_agent_groups(), 0);
        assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
    }

    // (c) H3 - registering the same wg/role drops the prior Terminated duplicate row.
    #[test]
    fn register_dedups_prior_terminated_row_for_same_identity() {
        let (state, backend) = state_with_fake();
        let root_a = identity(70, 70);
        backend.add_tree(root_a, vec![observed(70, 70, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        let id_a = Uuid::new_v4();
        state
            .register_group(
                permit,
                id_a,
                "agent".into(),
                None,
                None,
                Some("wg-1".into()),
                Some("tech-lead".into()),
                None,
                root_a,
            )
            .unwrap();
        let killed = state.kill_group(id_a, ResourceKillReason::User).unwrap();
        assert_eq!(killed.state, ResourceGroupState::Terminated);

        // Relaunch the same wg/role with a fresh pid: the prior Terminated row is dropped.
        let root_b = identity(71, 71);
        backend.add_tree(root_b, vec![observed(71, 71, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        let id_b = Uuid::new_v4();
        state
            .register_group(
                permit,
                id_b,
                "agent".into(),
                None,
                None,
                Some("wg-1".into()),
                Some("tech-lead".into()),
                None,
                root_b,
            )
            .unwrap();

        let snap = state.snapshot(limits(3));
        assert_eq!(snap.groups.len(), 1);
        assert_eq!(snap.groups[0].session_id, id_b.to_string());
        assert_eq!(snap.groups[0].state, ResourceGroupState::Running);
    }

    // (c2) #566 - two different projects running the same wg/role must NOT dedup
    // against each other's Terminated row (the dedup predicate gained `project`).
    // Regression guard: reverting the `g.project == project` clause makes this
    // assert len == 1 instead of 2.
    #[test]
    fn cross_project_same_role_terminated_rows_are_not_deduped() {
        let (state, backend) = state_with_fake();

        // Project A: register the (wg-1, tech-lead) group, then kill it so its
        // row is Terminated.
        let root_a = identity(80, 80);
        backend.add_tree(root_a, vec![observed(80, 80, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        let id_a = Uuid::new_v4();
        state
            .register_group(
                permit,
                id_a,
                "agent".into(),
                None,
                None,
                Some("wg-1".into()),
                Some("tech-lead".into()),
                Some("proj-a".into()),
                root_a,
            )
            .unwrap();
        let killed = state.kill_group(id_a, ResourceKillReason::User).unwrap();
        assert_eq!(killed.state, ResourceGroupState::Terminated);

        // Project B: same wg/role, different project. The predicate's
        // `g.project == project` clause means A's Terminated row is NOT dropped.
        let root_b = identity(81, 81);
        backend.add_tree(root_b, vec![observed(81, 81, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
        let id_b = Uuid::new_v4();
        state
            .register_group(
                permit,
                id_b,
                "agent".into(),
                None,
                None,
                Some("wg-1".into()),
                Some("tech-lead".into()),
                Some("proj-b".into()),
                root_b,
            )
            .unwrap();

        // Both rows survive: A (Terminated, proj-a) + B (Running, proj-b).
        let snap = state.snapshot(limits(3));
        assert_eq!(snap.groups.len(), 2);
    }

    // (T2) H1 - the reaping sample stamps the friendly message and suppresses the raw
    // warning, while the non-reaping strike still surfaces it.
    #[test]
    fn reaping_sample_sets_friendly_message_and_suppresses_warning() {
        let (state, backend) = state_with_fake();
        let root = identity(72, 72);
        backend.add_tree(root, vec![observed(72, 72, None, 0)]);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();

        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        backend.mark_gone(root);

        // First strike (non-reaping): the raw warning IS surfaced.
        let first = state.snapshot(limits(1));
        assert!(first
            .warnings
            .iter()
            .any(|w| w.contains("was not in process snapshot")));

        // Second strike reaps: friendly last_error, raw warning suppressed.
        let reaping = state.snapshot(limits(1));
        assert!(reaping
            .warnings
            .iter()
            .all(|w| !w.contains("was not in process snapshot")));
        let row = reaping
            .groups
            .iter()
            .find(|g| g.session_id == id.to_string())
            .expect("reaped group present in snapshot");
        assert_eq!(row.state, ResourceGroupState::Terminated);
        assert_eq!(
            row.last_error.as_deref(),
            Some("agent process exited; resource slot reclaimed")
        );
    }

    // (T3) H1 never races H2: a Quarantined group is never sampled, so it is never reaped.
    #[test]
    fn reap_never_touches_quarantined_group() {
        let (state, backend) = state_with_fake();
        let root = identity(73, 73);
        let child = identity(74, 74);
        backend.add_tree(
            root,
            vec![observed(73, 73, None, 0), observed(74, 74, Some(73), 1)],
        );
        backend.mark_stubborn(child);
        let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
        let id = Uuid::new_v4();
        state
            .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
            .unwrap();
        let result = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert!(result.quarantined);

        // Even with a missing-root signal + confirmed-gone root, sample_running_groups
        // skips Quarantined, so H1 never reaps it to Terminated.
        backend.replace_tree(root, Vec::new(), missing_root_error(root));
        backend.mark_gone(root);
        state.snapshot(limits(1));
        state.snapshot(limits(1));
        assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
        assert_eq!(state.active_agent_groups(), 1);
    }

    // (T4) H3 - retained Terminated rows are bounded by MAX_TERMINATED_RETAINED.
    #[test]
    fn terminated_rows_are_bounded_by_retention_cap() {
        let (state, backend) = state_with_fake();
        let total = MAX_TERMINATED_RETAINED + 4;
        for i in 0..total {
            let pid = 100 + i as u32;
            let root = identity(pid, pid as u64);
            backend.add_tree(root, vec![observed(pid, pid as u64, None, 0)]);
            let permit = state.try_reserve_agent_slot(limits(100)).unwrap().unwrap();
            let id = Uuid::new_v4();
            state
                .register_group(permit, id, "agent".into(), None, None, None, None, None, root)
                .unwrap();
            let killed = state.kill_group(id, ResourceKillReason::User).unwrap();
            assert_eq!(killed.state, ResourceGroupState::Terminated);
        }

        let snap = state.snapshot(limits(100));
        assert_eq!(snap.groups.len(), MAX_TERMINATED_RETAINED);
        assert_eq!(state.active_agent_groups(), 0);
    }
}
