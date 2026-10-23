use super::*;
use crate::resource_monitor::types::{ResourceGroupWarningLevel, ResourceOverallState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod admission;
mod groups;
mod reaping;

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
    /// #1151 - call counters so a test can assert that an ineligible orphan retry
    /// touched the backend zero times.
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
}

struct CapabilityTestBackend {
    supported: AtomicBool,
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
    current_process_memory_calls: AtomicUsize,
}

impl CapabilityTestBackend {
    fn new(supported: bool) -> Self {
        Self {
            supported: AtomicBool::new(supported),
            observe_tree_calls: AtomicUsize::new(0),
            observe_identity_calls: AtomicUsize::new(0),
            terminate_verified_calls: AtomicUsize::new(0),
            current_process_memory_calls: AtomicUsize::new(0),
        }
    }

    fn set_supported(&self, supported: bool) {
        self.supported.store(supported, Ordering::SeqCst);
    }
}

impl ProcessTreeBackend for CapabilityTestBackend {
    fn supports_process_tree_enforcement(&self) -> bool {
        self.supported.load(Ordering::SeqCst)
    }

    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ObservedProcessTree::default())
    }

    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(ProcessIdentity {
            pid,
            creation_time_100ns: u64::from(pid),
        }))
    }

    fn terminate_verified(
        &self,
        _process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
        Ok(TerminateOutcome::AlreadyGone)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        self.current_process_memory_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(ProcessMemory::default())
    }
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

    fn mark_remove_on_terminate(&self, trigger: ProcessIdentity, removed: Vec<ProcessIdentity>) {
        self.remove_on_terminate
            .lock()
            .unwrap()
            .insert(trigger, removed);
    }

    fn terminated(&self) -> Vec<ProcessIdentity> {
        self.terminated.lock().unwrap().clone()
    }

    /// #1151 - (observe_tree, observe_identity, terminate_verified) call counts.
    fn call_counts(&self) -> (usize, usize, usize) {
        (
            self.observe_tree_calls.load(Ordering::SeqCst),
            self.observe_identity_calls.load(Ordering::SeqCst),
            self.terminate_verified_calls.load(Ordering::SeqCst),
        )
    }
}

impl ProcessTreeBackend for FakeProcessTreeBackend {
    fn observe_tree(&self, root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
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
        self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
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
        self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
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

fn assert_disabled_snapshot(snapshot: &ResourceSnapshot, expected_limits: ResourceLimits) {
    assert_eq!(snapshot.overall_state, ResourceOverallState::Unknown);
    assert!(!snapshot.monitor_enabled);
    assert_eq!(snapshot.active_agent_groups, 0);
    assert_eq!(
        snapshot.max_concurrent_agent_groups,
        expected_limits.max_concurrent_agent_processes
    );
    assert_eq!(snapshot.app_private_bytes, None);
    assert_eq!(snapshot.app_working_set_bytes, None);
    assert!(snapshot.groups.is_empty());
    assert!(snapshot.warnings.is_empty());
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

    /// #1151 - deterministic stand-in for a concurrent transition, so R6 can drive
    /// the Terminating classification without threads. Stays inside `mod tests`
    /// because its only caller is in this file.
    fn test_force_state(&self, id: Uuid, state: ResourceGroupState) {
        if let Some(group) = self.inner.lock().unwrap().groups.get_mut(&id) {
            group.state = state;
        }
    }
}

fn missing_root_error(root: ProcessIdentity) -> Vec<String> {
    vec![format!("root pid {} was not in process snapshot", root.pid)]
}

// (a) H1 - a naturally-exited root reaps after two strikes + a confirmed-gone check.

/// #1151 - registers a group whose cleanup quarantines on a stubborn child, which is
/// the shape every orphan takes: the root and one descendant, the descendant refusing
/// to verify gone at teardown. Returns the group's root and child identities.
fn quarantined_group(
    state: &ResourceMonitorState,
    backend: &FakeProcessTreeBackend,
    id: Uuid,
    root_pid: u32,
    max: u32,
    identity_triple: Option<(&str, &str, &str)>,
) -> (ProcessIdentity, ProcessIdentity) {
    let root = identity(root_pid, u64::from(root_pid));
    let child = identity(root_pid + 1, u64::from(root_pid + 1));
    backend.add_tree(
        root,
        vec![
            observed(root_pid, u64::from(root_pid), None, 0),
            observed(root_pid + 1, u64::from(root_pid + 1), Some(root_pid), 1),
        ],
    );
    backend.mark_stubborn(child);
    let (workgroup, agent, project) = match identity_triple {
        Some((workgroup, agent, project)) => (
            Some(workgroup.to_string()),
            Some(agent.to_string()),
            Some(project.to_string()),
        ),
        None => (None, None, None),
    };
    let permit = state.try_reserve_agent_slot(limits(max)).unwrap().unwrap();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            workgroup,
            agent,
            project,
            root,
        )
        .unwrap();
    let first = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert!(first.quarantined);
    assert_eq!(first.state, ResourceGroupState::Quarantined);
    (root, child)
}
