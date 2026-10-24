use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use uuid::Uuid;

use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceGroupState,
    ResourceKillReason, ResourceKillResult, ResourceLaunchMetadata, ResourceLimits,
    ResourceSnapshot, TerminateOutcome,
};

mod sampling;
mod snapshot;
mod termination;

use snapshot::{build_snapshot, disabled_snapshot};

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

#[cfg(test)]
mod tests;
