use super::*;
use crate::resource_monitor::types::{ResourceGroupWarningLevel, ResourceOverallState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

#[test]
fn unsupported_backend_never_consumes_agent_capacity() {
    let backend = Arc::new(CapabilityTestBackend::new(false));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    let test_limits = limits(1);

    for _ in 0..33 {
        assert!(state.try_reserve_agent_slot(test_limits).unwrap().is_none());
        assert_eq!(state.active_agent_groups(), 0);
        assert_disabled_snapshot(&state.snapshot(test_limits), test_limits);
    }

    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn supported_backend_still_enforces_agent_capacity() {
    let (state, _backend) = state_with_fake();
    let first = state
        .try_reserve_agent_slot(limits(1))
        .unwrap()
        .expect("default-supported backend reserves the first slot");

    let error = state.try_reserve_agent_slot(limits(1)).unwrap_err();
    assert!(error.contains("1/1"), "unexpected cap error: {error}");

    state.release_unregistered_permit(first);
    let next = state
        .try_reserve_agent_slot(limits(1))
        .unwrap()
        .expect("released default-supported permit restores capacity");
    state.release_unregistered_permit(next);
}

#[test]
fn unsupported_backend_releases_preissued_permits_before_registration() {
    let backend = Arc::new(CapabilityTestBackend::new(true));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    let test_limits = limits(1);

    let logical_permit = state.try_reserve_agent_slot(test_limits).unwrap().unwrap();
    backend.set_supported(false);
    assert!(state.hold_logical_agent_slot(logical_permit).is_none());
    backend.set_supported(true);
    let replacement = state
        .try_reserve_agent_slot(test_limits)
        .unwrap()
        .expect("logical-slot rejection releases the preissued permit");
    state.release_unregistered_permit(replacement);

    let registration_permit = state.try_reserve_agent_slot(test_limits).unwrap().unwrap();
    let session_id = Uuid::new_v4();
    let mut registration = ResourceLaunchRegistration::new(
        state.clone(),
        registration_permit,
        ResourceLaunchMetadata {
            session_id,
            name: "agent".to_string(),
            agent_id: None,
            agent_label: None,
            workgroup: None,
            agent: None,
            project: None,
        },
    );
    backend.set_supported(false);
    assert_eq!(registration.register_root_pid(77).unwrap(), None);
    assert!(!state.has_registered_group(session_id));
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);

    backend.set_supported(true);
    let replacement = state
        .try_reserve_agent_slot(test_limits)
        .unwrap()
        .expect("registration rejection releases the preissued permit");
    state.release_unregistered_permit(replacement);
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
        .register_group(
            permit,
            Uuid::new_v4(),
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let snapshot = state.snapshot(limits(3));
    assert_eq!(snapshot.groups[0].process_count, 2);
    assert!(snapshot.groups[0].descendants_observed);
    assert_eq!(snapshot.overall_state, ResourceOverallState::Ok);
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
#[test]
fn natural_exit_reaps_after_two_strikes_and_confirm() {
    let (state, backend) = state_with_fake();
    let root = identity(60, 60);
    backend.add_tree(root, vec![observed(60, 60, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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

// #1151 (R1) - the fix itself at registry level: once the blocker is verifiably gone,
// the registry-only retry releases the permit and the next launch is admitted.
#[test]
fn orphan_quarantine_retry_reclaims_slot() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, child) = quarantined_group(&state, &backend, id, 80, 1, None);
    assert_eq!(state.active_agent_groups(), 1);

    backend.mark_gone(child);
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    let QuarantineRetryOutcome::Completed(result) = outcome else {
        panic!("an eligible orphan retry runs the cleanup: {outcome:?}");
    };
    assert!(!result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Terminated);
    assert!(!state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 0);
    assert!(state.try_reserve_agent_slot(limits(1)).is_ok());
}

// #1151 (R2) - the safety half of R1: an owned descendant that is still unverifiable
// keeps blocking capacity. The retry decides only WHETHER kill_group runs.
#[test]
fn orphan_retry_keeps_live_descendant_quarantined() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 82, 1, None);

    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    let QuarantineRetryOutcome::Completed(result) = outcome else {
        panic!("an eligible orphan retry runs the cleanup: {outcome:?}");
    };
    assert!(result.quarantined);
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);
    assert!(state.try_reserve_agent_slot(limits(1)).is_err());
}

// #1151 (R3) - a stale sample bound to a different root identity cannot act.
#[test]
fn orphan_retry_rejects_root_mismatch() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (_root, _child) = quarantined_group(&state, &backend, id, 84, 1, None);

    let before = backend.call_counts();
    let outcome = state
        .retry_orphaned_quarantine(id, identity(999, 999))
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::RootMismatch)
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R4) - a Running group is never taken by the orphan path. That case belongs
// to the missing-root strike reaper (#1040), not to this fix.
#[test]
fn orphan_retry_rejects_non_quarantined_state() {
    let (state, backend) = state_with_fake();
    let root = identity(86, 86);
    backend.add_tree(root, vec![observed(86, 86, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Running
        ))
    ));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R5) - defensive arm only. A Quarantined row can never be removed from the
// group map, so this is unreachable through real sampling and must be driven with a
// fabricated UUID.
#[test]
fn orphan_retry_rejects_unregistered() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 88, 1, None);

    let before = backend.call_counts();
    let outcome = state
        .retry_orphaned_quarantine(Uuid::new_v4(), root)
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotRegistered)
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R6) - deterministic stand-in for the concurrent-kill case: another caller
// already owns the transition, so the orphan retry must not touch it.
#[test]
fn orphan_retry_rejects_terminating_state() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, _child) = quarantined_group(&state, &backend, id, 90, 1, None);
    state.test_force_state(id, ResourceGroupState::Terminating);

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Terminating
        ))
    ));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminating));
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R7) - a second retry after a successful one is a no-op. The watchdog
// re-samples every tick, so this runs in production whenever a tick overlaps a
// just-completed reclaim.
#[test]
fn orphan_retry_is_idempotent_after_success() {
    let (state, backend) = state_with_fake();
    let id = Uuid::new_v4();
    let (root, child) = quarantined_group(&state, &backend, id, 92, 1, None);

    backend.mark_gone(child);
    assert!(matches!(
        state.retry_orphaned_quarantine(id, root).unwrap(),
        QuarantineRetryOutcome::Completed(_)
    ));
    assert_eq!(state.active_agent_groups(), 0);

    let before = backend.call_counts();
    let outcome = state.retry_orphaned_quarantine(id, root).unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::NotQuarantined(
            ResourceGroupState::Terminated
        ))
    ));
    assert_eq!(state.active_agent_groups(), 0);
    assert_eq!(backend.call_counts(), before);
}

// #1151 (R8) - defense in depth. The watchdog is already gated on capability, so this
// check is unreachable on Windows; it must still cost nothing where it is reachable.
#[test]
fn unsupported_backend_orphan_retry_is_noop() {
    let backend = Arc::new(CapabilityTestBackend::new(false));
    let state = ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);

    let outcome = state
        .retry_orphaned_quarantine(Uuid::new_v4(), identity(94, 94))
        .unwrap();
    assert!(matches!(
        outcome,
        QuarantineRetryOutcome::Skipped(QuarantineRetrySkip::Unsupported)
    ));
    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}

// #1151 (R9) - group_counts_toward_admission is the admission predicate itself, not
// an inference, so the watchdog can report the fact without claiming authorship.
// Compared against the active_count FILTER (the delta this group contributes), not
// against active_agent_groups() directly, which also counts pending permits.
#[test]
fn group_counts_toward_admission_matches_the_active_count_filter() {
    let (state, backend) = state_with_fake();
    let absent = Uuid::new_v4();
    assert_eq!(state.active_agent_groups(), 0);
    assert!(!state.group_counts_toward_admission(absent));

    let root = identity(96, 96);
    let child = identity(97, 97);
    backend.add_tree(
        root,
        vec![observed(96, 96, None, 0), observed(97, 97, Some(96), 1)],
    );
    backend.mark_stubborn(child);
    let permit = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // Running: included by the filter, so the figure rises by exactly this group.
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);

    // Quarantined: still included. This is the whole reason the permit leaks.
    state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert!(state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 1);

    // Terminated with the permit released: excluded, back to the empty figure.
    backend.mark_gone(child);
    state.retry_orphaned_quarantine(id, root).unwrap();
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Terminated));
    assert!(!state.group_counts_toward_admission(id));
    assert_eq!(state.active_agent_groups(), 0);

    // Absent: never included.
    assert!(!state.group_counts_toward_admission(absent));
}

// #1151 (R10) - the restart shape at registry level. A restart replacement carries the
// same workgroup/agent/project under a NEW uuid, so register_group's relaunch dedup
// WOULD have collected the old row and does not, because it is Quarantined rather
// than Terminated. That is the accumulator; the retry is what drains it.
#[test]
fn restart_shaped_orphan_retry_unblocks_relaunch_dedup() {
    let (state, backend) = state_with_fake();
    let triple = ("wg-1", "dev-rust", "ac");
    let wg = || Some(triple.0.to_string());
    let agent = || Some(triple.1.to_string());
    let project = || Some(triple.2.to_string());

    let id_a = Uuid::new_v4();
    let (root_a, child_a) = quarantined_group(&state, &backend, id_a, 100, 2, Some(triple));

    // The restart replacement: a NEW uuid with the SAME identity triple.
    let id_b = Uuid::new_v4();
    let root_b = identity(110, 110);
    backend.add_tree(root_b, vec![observed(110, 110, None, 0)]);
    let permit_b = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    state
        .register_group(
            permit_b,
            id_b,
            "agent".into(),
            None,
            None,
            wg(),
            agent(),
            project(),
            root_b,
        )
        .unwrap();

    // A's row survives the dedup because it is Quarantined, and both rows count.
    assert_eq!(
        state.test_state(id_a),
        Some(ResourceGroupState::Quarantined)
    );
    assert_eq!(state.active_agent_groups(), 2);

    // The blocker clears and the orphan retry drains A.
    backend.mark_gone(child_a);
    assert!(matches!(
        state.retry_orphaned_quarantine(id_a, root_a).unwrap(),
        QuarantineRetryOutcome::Completed(_)
    ));
    assert_eq!(state.test_state(id_a), Some(ResourceGroupState::Terminated));
    assert_eq!(state.active_agent_groups(), 1);

    // A third launch of the same triple now reclaims A's row instead of stacking.
    let id_c = Uuid::new_v4();
    let root_c = identity(120, 120);
    backend.add_tree(root_c, vec![observed(120, 120, None, 0)]);
    let permit_c = state.try_reserve_agent_slot(limits(2)).unwrap().unwrap();
    state
        .register_group(
            permit_c,
            id_c,
            "agent".into(),
            None,
            None,
            wg(),
            agent(),
            project(),
            root_c,
        )
        .unwrap();
    assert_eq!(state.test_state(id_a), None);
    assert_eq!(state.active_agent_groups(), 2);
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
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

// (T3) H1 never races H2: a Quarantined group IS sampled (#647 C) but merge_sample
// only strike-counts/nominates Running groups, so it is still never reaped.
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
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(result.quarantined);

    // Even with a missing-root signal + confirmed-gone root, merge_sample only
    // strike-counts/nominates Running groups, so H1 never reaps Quarantined to
    // Terminated (even though #647 C now re-samples it).
    backend.replace_tree(root, Vec::new(), missing_root_error(root));
    backend.mark_gone(root);
    state.snapshot(limits(1));
    state.snapshot(limits(1));
    assert_eq!(state.test_state(id), Some(ResourceGroupState::Quarantined));
    assert_eq!(state.active_agent_groups(), 1);
}

// #647 C: a Quarantined group IS included in sample_running_groups so its process
// list is re-observed each snapshot (unfreezing the display). Pairs with
// reap_never_touches_quarantined_group, which proves sampling it stays reap-safe.
#[test]
fn quarantined_group_is_resampled() {
    let (state, backend) = state_with_fake();
    let root = identity(81, 81);
    let child = identity(82, 82);
    backend.add_tree(
        root,
        vec![observed(81, 81, None, 0), observed(82, 82, Some(81), 1)],
    );
    backend.mark_stubborn(child);
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let result = state.kill_group(id, ResourceKillReason::User).unwrap();
    assert!(result.quarantined);

    let sampled = state.sample_running_groups();
    assert!(
        sampled.iter().any(|(sid, _)| *sid == id),
        "Quarantined group must be re-sampled (#647 C)"
    );
}

// #647 D / E1: the ACCESS_DENIED signature must match the EXACT win32 code 5, so
// unrelated codes that merely start with the digit 5 (50/53/567) never trip it.
#[test]
fn access_denied_signature_matches_exact_code_5_only() {
    assert!(has_access_denied_signature(&[
        "pid 1: TerminateProcess failed: win32 error 5".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: OpenProcess(1) failed: win32 error 50".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: win32 error 53".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: win32 error 567".to_string()
    ]));
    assert!(!has_access_denied_signature(&[
        "pid 1: timed out waiting for pid 1 to exit".to_string()
    ]));
    // `any` policy (E2): a mixed set with one error-5 still flags. HIGH-2 keeps the
    // per-PID detail in `message`, so this only ADDS the AV hint, never hides a failure.
    assert!(has_access_denied_signature(&[
        "pid 1: win32 error 5".to_string(),
        "pid 2: timed out".to_string(),
    ]));
}

// #647 D: blocked_by_security serializes as camelCase and, being serde(default),
// a legacy payload without the field deserializes to false.
#[test]
fn kill_result_blocked_by_security_round_trips_and_defaults() {
    let result = ResourceKillResult {
        session_id: "s".to_string(),
        state: ResourceGroupState::Quarantined,
        killed_processes: Vec::new(),
        quarantined: true,
        message: "pid 1: win32 error 5".to_string(),
        blocked_by_security: false,
        finalized: false,
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"blockedBySecurity\":false"));
    assert!(json.contains("\"finalized\":false"));

    // Strip the field to simulate a pre-#647 payload; serde(default) -> false.
    let legacy = json.replace(",\"blockedBySecurity\":false", "");
    assert!(!legacy.contains("blockedBySecurity"));
    let parsed: ResourceKillResult = serde_json::from_str(&legacy).unwrap();
    assert!(!parsed.blocked_by_security);
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
            .register_group(
                permit,
                id,
                "agent".into(),
                None,
                None,
                None,
                None,
                None,
                root,
            )
            .unwrap();
        let killed = state.kill_group(id, ResourceKillReason::User).unwrap();
        assert_eq!(killed.state, ResourceGroupState::Terminated);
    }

    let snap = state.snapshot(limits(100));
    assert_eq!(snap.groups.len(), MAX_TERMINATED_RETAINED);
    assert_eq!(state.active_agent_groups(), 0);
}

// #632 B2a - the app-shutdown bulk path (kill_all_owned_groups -> fresh_targets_only)
// targets only the fresh live tree, NOT the accumulated observed_processes. A child
// that left the live tree but is still alive must therefore NOT be killed here.
#[test]
fn kill_all_owned_groups_uses_fresh_tree_only() {
    let (state, backend) = state_with_fake();
    let root = identity(90, 90);
    let child = identity(91, 91);
    backend.add_tree(
        root,
        vec![observed(90, 90, None, 0), observed(91, 91, Some(90), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    // register seeded observed_processes with {root, child}. The child now leaves
    // the live tree (fresh observe_tree is root-only) but stays alive.
    backend.replace_tree(root, vec![observed(90, 90, None, 0)], Vec::new());
    backend.mark_present(child);

    let results = state.kill_all_owned_groups(ResourceKillReason::AppShutdown);
    assert_eq!(results.len(), 1);
    assert!(!results[0].quarantined);
    assert!(backend.terminated().contains(&root));
    assert!(
        !backend.terminated().contains(&child),
        "fresh-only shutdown kill must not target the accumulated child"
    );
}

// #632 B2a contrast - the per-session union path (kill_group, fresh_targets_only =
// false) DOES target the accumulated child. Same setup as the fresh-only test;
// proves the flag is the only difference.
#[test]
fn kill_group_union_still_targets_accumulated_child() {
    let (state, backend) = state_with_fake();
    let root = identity(92, 92);
    let child = identity(93, 93);
    backend.add_tree(
        root,
        vec![observed(92, 92, None, 0), observed(93, 93, Some(92), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();

    backend.replace_tree(root, vec![observed(92, 92, None, 0)], Vec::new());
    backend.mark_present(child);

    let result = state
        .kill_group(id, ResourceKillReason::SessionDestroy)
        .unwrap();
    assert!(!result.quarantined);
    assert!(backend.terminated().contains(&root));
    assert!(
        backend.terminated().contains(&child),
        "union kill must still target the accumulated child"
    );
}

// #632 B2b - a CLEAN sample (root alive, no errors) prunes a descendant that has
// exited, so observed_processes tracks the live set instead of accumulating.
#[test]
fn clean_sample_prunes_exited_descendant() {
    let (state, backend) = state_with_fake();
    let root = identity(94, 94);
    let child = identity(95, 95);
    backend.add_tree(
        root,
        vec![observed(94, 94, None, 0), observed(95, 95, Some(94), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    // Child exits; a clean root-only sample prunes it.
    backend.replace_tree(root, vec![observed(94, 94, None, 0)], Vec::new());
    backend.mark_gone(child);
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 1);
}

// #632 B2b guard - a missing-root sample (empty tree + missing-root error) must NOT
// prune; the last-known set is owned by the #559 strike/reap path.
#[test]
fn missing_root_sample_retains_last_known_set() {
    let (state, backend) = state_with_fake();
    let root = identity(96, 96);
    backend.add_tree(
        root,
        vec![observed(96, 96, None, 0), observed(97, 97, Some(96), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    backend.replace_tree(
        root,
        Vec::new(),
        vec![format!("root pid {} was not in process snapshot", root.pid)],
    );
    backend.mark_gone(root);
    let snap = state.snapshot(limits(1));
    assert_eq!(
        snap.groups[0].process_count, 2,
        "missing-root sample must retain the last-known set"
    );
}

// #632 B2b guard (MED-3) - a root-alive but ERRORED sample (e.g. a transient
// "identity unavailable" for a child) must NOT prune, so a racy/partial snapshot
// can never drop a still-live descendant.
#[test]
fn errored_sample_does_not_prune() {
    let (state, backend) = state_with_fake();
    let root = identity(98, 98);
    let child = identity(99, 99);
    backend.add_tree(
        root,
        vec![observed(98, 98, None, 0), observed(99, 99, Some(98), 1)],
    );
    let permit = state.try_reserve_agent_slot(limits(1)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "agent".into(),
            None,
            None,
            None,
            None,
            None,
            root,
        )
        .unwrap();
    let snap = state.snapshot(limits(1));
    assert_eq!(snap.groups[0].process_count, 2);

    // Root-only fresh tree, but the sample carries an enumeration error: root_alive
    // is true yet sample_clean is false, so MED-3 keeps the live child.
    backend.replace_tree(
        root,
        vec![observed(98, 98, None, 0)],
        vec![format!("identity unavailable for pid {}", child.pid)],
    );
    backend.mark_present(child);
    let snap = state.snapshot(limits(1));
    assert_eq!(
        snap.groups[0].process_count, 2,
        "errored sample must not prune the live child"
    );
}

// #2581 - per-agent memory warnings are structured (session id + level +
// bytes + limit) and never duplicated as free text in `warnings`.
fn snapshot_with_group_at(bytes: u32) -> (Uuid, ResourceSnapshot) {
    let (state, backend) = state_with_fake();
    let root = identity(bytes, bytes as u64);
    backend.add_tree(root, vec![observed(bytes, bytes as u64, None, 0)]);
    let permit = state.try_reserve_agent_slot(limits(3)).unwrap().unwrap();
    let id = Uuid::new_v4();
    state
        .register_group(
            permit,
            id,
            "Claude Code".into(),
            None,
            None,
            Some("room-1-team".into()),
            Some("dev-rust".into()),
            None,
            root,
        )
        .unwrap();
    (id, state.snapshot(limits(3)))
}

#[test]
fn group_over_warn_limit_emits_structured_group_warning() {
    let (id, snap) = snapshot_with_group_at(150);
    assert_eq!(snap.group_warnings.len(), 1);
    let warning = &snap.group_warnings[0];
    assert_eq!(warning.session_id, id.to_string());
    assert_eq!(warning.level, ResourceGroupWarningLevel::Warn);
    assert_eq!(warning.private_bytes, 150);
    assert_eq!(warning.limit_bytes, 100);
    assert!(snap.warnings.iter().all(|w| !w.contains("exceeds group")));
    assert_eq!(snap.overall_state, ResourceOverallState::Warn);
}

#[test]
fn group_over_kill_limit_emits_kill_group_warning() {
    let (id, snap) = snapshot_with_group_at(250);
    assert_eq!(snap.group_warnings.len(), 1);
    let warning = &snap.group_warnings[0];
    assert_eq!(warning.session_id, id.to_string());
    assert_eq!(warning.level, ResourceGroupWarningLevel::Kill);
    assert_eq!(warning.private_bytes, 250);
    assert_eq!(warning.limit_bytes, 200);
    assert!(snap.warnings.iter().all(|w| !w.contains("exceeds group")));
    assert_eq!(snap.overall_state, ResourceOverallState::Critical);
}

#[test]
fn group_under_warn_limit_emits_no_group_warning() {
    let (_id, snap) = snapshot_with_group_at(50);
    assert!(snap.group_warnings.is_empty());
    assert_eq!(snap.overall_state, ResourceOverallState::Ok);
}

#[test]
fn disabled_snapshot_has_no_group_warnings() {
    assert!(disabled_snapshot(limits(1)).group_warnings.is_empty());
}

#[test]
fn group_warnings_serialize_in_camel_case() {
    let (id, snap) = snapshot_with_group_at(150);
    let value = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        value["groupWarnings"],
        serde_json::json!([{
            "sessionId": id.to_string(),
            "level": "warn",
            "privateBytes": 150,
            "limitBytes": 100,
        }])
    );
}

#[test]
fn snapshot_without_group_warnings_key_deserializes_to_empty() {
    let mut value = serde_json::to_value(disabled_snapshot(limits(1))).unwrap();
    value.as_object_mut().unwrap().remove("groupWarnings");
    let snap: ResourceSnapshot = serde_json::from_value(value).unwrap();
    assert!(snap.group_warnings.is_empty());
}
