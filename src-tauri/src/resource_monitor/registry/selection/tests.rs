use super::super::termination::{CleanupFailure, DiagnosticTerminationError};
use super::super::{AgentLaunchPermit, ProcessTreeBackend, ResourceError, ResourceMonitorState};
use super::*;
use crate::cleanup_diagnostics::operation::{EvidenceWriter, Operation};
use crate::cleanup_diagnostics::schema::{EvidenceError, EvidenceEvent, EvidenceRecord};
use crate::cleanup_diagnostics::store::{Clock, Diagnostics};
use crate::resource_monitor::types::{
    ObservedProcessTree, ProcessMemory, ResourceGroupState, ResourceKillReason, TerminateOutcome,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(Default)]
struct Recording {
    events: Vec<EvidenceEvent>,
    synced: usize,
    fail: Option<&'static str>,
    index: Option<usize>,
    footer_attempts: usize,
    on_fail: Option<Box<dyn FnMut() + Send>>,
}

struct Writer(Arc<Mutex<Recording>>);
impl EvidenceWriter for Writer {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        let (name, index) = match &record.event {
            EvidenceEvent::OperationBegin => ("header", None),
            EvidenceEvent::Target { row } => ("target", Some(row.index)),
            EvidenceEvent::SelectionComplete { .. } => ("selection", None),
            EvidenceEvent::PreCall { index, .. } => ("pre_call", Some(*index)),
            EvidenceEvent::ApiReturn { index, .. } => ("api_return", Some(*index)),
            EvidenceEvent::ExitObserved { index, .. } => ("exit", Some(*index)),
            EvidenceEvent::OperationComplete { .. } => {
                state.footer_attempts += 1;
                ("footer", None)
            }
        };
        if state.fail == Some(name) && (state.index.is_none() || state.index == index) {
            if let Some(hook) = state.on_fail.as_mut() {
                hook();
            }
            return Err(EvidenceError::new("fake", "injected write failure"));
        }
        state.events.push(record.event.clone());
        Ok(())
    }
    fn flush_sync(&mut self) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        state.synced = state.events.len();
        Ok(())
    }
    fn complete(&mut self) -> Result<(), EvidenceError> {
        Ok(())
    }
}

struct FixedClock;
impl Clock for FixedClock {
    fn utc(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap()
    }
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
}

struct Backend {
    processes: Mutex<Vec<ObservedProcess>>,
    alive: Mutex<BTreeMap<u32, ProcessIdentity>>,
    calls: Mutex<Vec<ProcessIdentity>>,
    samples: Mutex<usize>,
    forbid_kill: bool,
    ordinary_error: bool,
    recording: Arc<Mutex<Recording>>,
}
impl ProcessTreeBackend for Backend {
    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        *self.samples.lock().unwrap() += 1;
        Ok(ObservedProcessTree {
            processes: self.processes.lock().unwrap().clone(),
            errors: Vec::new(),
        })
    }
    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        if pid == 999 {
            return Err("unreadable".into());
        }
        Ok(self.alive.lock().unwrap().get(&pid).copied())
    }
    fn terminate_verified(
        &self,
        process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        assert!(!self.forbid_kill, "inspect reached a mutator");
        let recording = self.recording.lock().unwrap();
        if !recording.events.is_empty() {
            assert!(recording.events[..recording.synced]
                .iter()
                .any(|event| matches!(event, EvidenceEvent::SelectionComplete { .. })));
        }
        drop(recording);
        self.calls.lock().unwrap().push(process.identity);
        if self.ordinary_error {
            return Err("ordinary termination failure".into());
        }
        self.alive.lock().unwrap().remove(&process.identity.pid);
        Ok(TerminateOutcome::Terminated)
    }
    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

fn process(pid: u32, depth: u32) -> ObservedProcess {
    ObservedProcess {
        identity: ProcessIdentity {
            pid,
            creation_time_100ns: u64::from(pid) * 100,
        },
        parent_pid: None,
        parent_identity: None,
        exe_name: format!("retained-{pid}"),
        depth,
        private_bytes: None,
        working_set_bytes: None,
        cpu_percent: None,
        kill_allowed: true,
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    monitor: ResourceMonitorState,
    backend: Arc<Backend>,
    recording: Arc<Mutex<Recording>>,
    id: Uuid,
}
impl Fixture {
    fn new(fail: Option<&'static str>, forbid_kill: bool, ordinary_error: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let recording = Arc::new(Mutex::new(Recording {
            fail,
            ..Recording::default()
        }));
        let output = Arc::clone(&recording);
        let diagnostics = Diagnostics::with_test_ports(
            directory.path(),
            Arc::new(FixedClock),
            Arc::new(move || Box::new(Writer(Arc::clone(&output)))),
        )
        .unwrap();
        let processes = vec![process(10, 0), process(20, 1)];
        let backend = Arc::new(Backend {
            alive: Mutex::new(
                processes
                    .iter()
                    .map(|p| (p.identity.pid, p.identity))
                    .collect(),
            ),
            processes: Mutex::new(processes),
            calls: Mutex::new(Vec::new()),
            samples: Mutex::new(0),
            forbid_kill,
            ordinary_error,
            recording: Arc::clone(&recording),
        });
        let monitor = ResourceMonitorState::with_backend_spawn_and_diagnostics(
            backend.clone(),
            Arc::new(crate::pty::spawn_diagnostics::SpawnRegistry::new()),
            Arc::new(diagnostics),
        );
        let id = Uuid::new_v4();
        monitor.inner.lock().unwrap().pending_permits.insert(1);
        monitor
            .register_group(
                AgentLaunchPermit { generation: 1 },
                id,
                "fake".into(),
                None,
                None,
                None,
                None,
                None,
                process(10, 0).identity,
            )
            .unwrap();
        Self {
            _directory: directory,
            monitor,
            backend,
            recording,
            id,
        }
    }
    fn binding(&self) -> schema::Binding {
        let inner = self.monitor.inner.lock().unwrap();
        let group = &inner.groups[&self.id];
        schema::Binding {
            run_id: self.monitor.diagnostics.run_id(),
            session_id: self.id,
            root: Some(identity(group.root_identity)),
            registration_generation: group.generation,
        }
    }
    fn arm(&self) {
        let reservation = self.monitor.diagnostics.reserve().unwrap();
        self.monitor
            .diagnostics
            .publish_arm_metadata(reservation, self.binding())
            .unwrap();
    }
    #[allow(clippy::result_large_err)]
    fn run(&self) -> Result<crate::resource_monitor::types::ResourceKillResult, CleanupFailure> {
        self.monitor
            .kill_group_diagnostic(self.id, ResourceKillReason::User)
    }
    fn state(&self) -> ResourceGroupState {
        self.monitor.inner.lock().unwrap().groups[&self.id].state
    }
    fn operation(&self) -> (Operation, Selection) {
        let operation = self
            .monitor
            .diagnostics
            .inspect_operation(self.binding())
            .unwrap();
        let targets = self.backend.processes.lock().unwrap().clone();
        let selection = select_targets(&targets, &targets);
        operation.selection(&selection.rows).unwrap();
        (operation, selection)
    }
}

#[test]
fn issue_3530_retained_fresh_order() {
    let root = process(10, 0);
    let mut child = process(20, 1);
    child.parent_identity = Some(root.identity);
    let retained = vec![root.clone(), child.clone(), process(30, 2)];
    let mut fresh_child = child.clone();
    fresh_child.exe_name = "fresh-child".into();
    let selection = select_targets(&retained, &[root, fresh_child]);
    assert_eq!(
        selection
            .targets
            .iter()
            .map(|p| p.identity.pid)
            .collect::<Vec<_>>(),
        [30, 20, 10]
    );
    assert_eq!(selection.rows[0].provenance, schema::Provenance::Retained);
    assert_eq!(selection.rows[1].provenance, schema::Provenance::Both);
    assert_eq!(
        selection.rows[1]
            .retained
            .as_ref()
            .unwrap()
            .executable
            .as_deref(),
        Some("retained-20")
    );
    assert_eq!(
        selection.rows[1].fresh.as_ref().unwrap().chain,
        [identity(process(10, 0).identity)]
    );
    assert_eq!(selection.targets[1].exe_name, "fresh-child");
}

#[test]
fn issue_3530_same_inputs_same_selection() {
    let inputs = vec![process(10, 2), process(20, 2)];
    let first = select_targets(&inputs, &inputs);
    let second = select_targets(&inputs, &inputs);
    assert_eq!(first.rows, second.rows);
    assert_eq!(first.rows[0].expected.pid, 20);
}

#[test]
fn issue_3530_inspect_never_mutates() {
    let f = Fixture::new(None, true, false);
    f.arm();
    let binding = f.binding();
    f.backend.processes.lock().unwrap().push(process(30, 2));
    let samples = *f.backend.samples.lock().unwrap();
    let receipt = f.monitor.inspect_group(f.id).unwrap();
    assert_eq!(receipt.status, "complete");
    assert_eq!(*f.backend.samples.lock().unwrap(), samples + 1);
    let inner = f.monitor.inner.lock().unwrap();
    assert_eq!(inner.groups[&f.id].observed_processes.len(), 2);
    assert_eq!(inner.groups[&f.id].state, ResourceGroupState::Running);
    assert!(inner.groups[&f.id].kill_started_at.is_none());
    drop(inner);
    assert!(f
        .monitor
        .diagnostics
        .arm_status_metadata(&binding)
        .is_some());
    assert_eq!(f.monitor.active_agent_groups(), 1);
    assert!(f.backend.calls.lock().unwrap().is_empty());
}

#[test]
fn issue_3530_diagnostic_two_rows() {
    let f = Fixture::new(None, false, false);
    f.arm();
    let result = f.run().unwrap();
    assert_eq!(
        result.killed_processes,
        [process(20, 1).identity, process(10, 0).identity]
    );
    assert_eq!(f.state(), ResourceGroupState::Terminated);
    let state = f.recording.lock().unwrap();
    let indexes: Vec<_> = state
        .events
        .iter()
        .filter_map(|event| match event {
            EvidenceEvent::PreCall { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indexes, [0, 1]);
    assert_eq!(state.footer_attempts, 1);
}

#[test]
fn issue_3530_selection_all_before_first_call() {
    let f = Fixture::new(None, false, false);
    f.arm();
    f.run().unwrap();
    let state = f.recording.lock().unwrap();
    let complete = state
        .events
        .iter()
        .position(|e| matches!(e, EvidenceEvent::SelectionComplete { count: 2 }))
        .unwrap();
    let first = state
        .events
        .iter()
        .position(|e| matches!(e, EvidenceEvent::PreCall { .. }))
        .unwrap();
    assert!(complete < first);
    assert_eq!(
        state.events[..complete]
            .iter()
            .filter(|e| matches!(e, EvidenceEvent::Target { .. }))
            .count(),
        2
    );
}

#[test]
fn issue_3530_diagnostic_altered_row() {
    let f = Fixture::new(None, false, false);
    let (operation, selection) = f.operation();
    let mut row = selection.rows[0].clone();
    row.rule = "altered".into();
    let error = f
        .backend
        .terminate_verified_diagnostic(&selection.targets[0], &row, &operation)
        .unwrap_err();
    assert!(matches!(error, DiagnosticTerminationError::Evidence(_)));
    assert!(f.backend.calls.lock().unwrap().is_empty());
}

#[test]
fn issue_3530_diagnostic_other_process() {
    let f = Fixture::new(None, false, false);
    let (operation, selection) = f.operation();
    let error = f
        .backend
        .terminate_verified_diagnostic(&selection.targets[1], &selection.rows[0], &operation)
        .unwrap_err();
    assert!(
        matches!(error, DiagnosticTerminationError::Evidence(error) if error.stage == "pre_call")
    );
    assert!(f.backend.calls.lock().unwrap().is_empty());
}

#[test]
fn issue_3530_diagnostic_skipped_indices() {
    let f = Fixture::new(None, false, false);
    f.backend.processes.lock().unwrap()[1].kill_allowed = false;
    f.arm();
    f.run().unwrap();
    let state = f.recording.lock().unwrap();
    let indexes: Vec<_> = state
        .events
        .iter()
        .filter_map(|event| match event {
            EvidenceEvent::PreCall { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indexes, [1]);
    assert_eq!(f.backend.calls.lock().unwrap().len(), 1);
}

#[test]
fn issue_3530_identity_reused_unreadable() {
    let f = Fixture::new(None, true, false);
    f.backend
        .alive
        .lock()
        .unwrap()
        .insert(20, process(20, 1).identity);
    f.backend
        .alive
        .lock()
        .unwrap()
        .get_mut(&20)
        .unwrap()
        .creation_time_100ns += 1;
    f.backend.processes.lock().unwrap().push(process(999, 2));
    f.monitor.inspect_group(f.id).unwrap();
    let state = f.recording.lock().unwrap();
    let rows: Vec<_> = state
        .events
        .iter()
        .filter_map(|event| match event {
            EvidenceEvent::Target { row } => Some(row),
            _ => None,
        })
        .collect();
    assert_eq!(rows[0].current.status, schema::IdentityStatus::Unreadable);
    assert_eq!(rows[1].current.status, schema::IdentityStatus::Reused);
    assert!(rows.iter().all(|row| row.current.executable.is_none()));
}

fn assert_veto(error: CleanupFailure, effects: bool) {
    match error {
        CleanupFailure::EvidenceVeto {
            result,
            effects_started,
            accounting_state,
            ..
        } => {
            assert_eq!(effects_started, effects);
            assert!(!result.finalized);
            assert!(result.quarantined);
            assert!(result.message.contains("evidence_gap"));
            assert_ne!(accounting_state, "unknown");
        }
        other @ CleanupFailure::Ordinary(_) => panic!("expected veto, got {other:?}"),
    }
}

#[test]
fn issue_3530_fail_before_api() {
    for stage in ["header", "selection", "pre_call"] {
        let f = Fixture::new(Some(stage), false, false);
        f.arm();
        assert_veto(f.run().unwrap_err(), false);
        assert!(f.backend.calls.lock().unwrap().is_empty());
        assert_eq!(f.state(), ResourceGroupState::Running);
        assert_eq!(f.monitor.active_agent_groups(), 1);
    }
}

#[test]
fn issue_3530_fail_after_api_stops_next() {
    let f = Fixture::new(Some("api_return"), false, false);
    f.arm();
    assert_veto(f.run().unwrap_err(), true);
    assert_eq!(f.backend.calls.lock().unwrap().len(), 1);
    assert_eq!(f.state(), ResourceGroupState::Quarantined);
    assert_eq!(f.monitor.active_agent_groups(), 1);
}

#[test]
fn issue_3530_diagnostic_row_failure_after_n() {
    let f = Fixture::new(Some("pre_call"), false, false);
    f.recording.lock().unwrap().index = Some(1);
    f.arm();
    match f.run().unwrap_err() {
        CleanupFailure::EvidenceVeto {
            effects_started,
            result,
            ..
        } => {
            assert!(effects_started);
            assert_eq!(result.killed_processes, [process(20, 1).identity]);
        }
        other @ CleanupFailure::Ordinary(_) => panic!("expected veto, got {other:?}"),
    }
    assert_eq!(f.backend.calls.lock().unwrap().len(), 1);
    assert_eq!(f.state(), ResourceGroupState::Quarantined);
}

#[test]
fn issue_3530_footer_failure_uses_effects_state() {
    let f = Fixture::new(Some("footer"), false, false);
    f.arm();
    assert_veto(f.run().unwrap_err(), true);
    assert_eq!(f.state(), ResourceGroupState::Quarantined);
    assert_eq!(f.recording.lock().unwrap().footer_attempts, 1);
    assert_eq!(f.monitor.active_agent_groups(), 1);
    let empty = Fixture::new(Some("footer"), false, false);
    empty
        .monitor
        .inner
        .lock()
        .unwrap()
        .groups
        .get_mut(&empty.id)
        .unwrap()
        .observed_processes
        .clear();
    empty.backend.processes.lock().unwrap().clear();
    empty.arm();
    assert_veto(empty.run().unwrap_err(), false);
    assert_eq!(empty.state(), ResourceGroupState::Running);
    assert_eq!(empty.recording.lock().unwrap().footer_attempts, 1);
}

#[test]
fn issue_3530_veto_running_no_new_retry_warn() {
    let f = Fixture::new(Some("selection"), false, false);
    f.arm();
    assert_veto(f.run().unwrap_err(), false);
    // Simulate elapsed backoff without sleeping; Running must still never be eligible.
    f.monitor.test_backdate_quarantine_retry(f.id);
    assert!(!f.monitor.quarantine_retry_due(f.id));
    let retry = f
        .monitor
        .retry_orphaned_quarantine(f.id, process(10, 0).identity)
        .unwrap();
    assert!(matches!(
        retry,
        super::super::QuarantineRetryOutcome::Skipped(_)
    ));
    assert!(f.backend.calls.lock().unwrap().is_empty());
}

#[test]
fn issue_3530_veto_preserves_prior_quarantine_backoff() {
    let f = Fixture::new(Some("pre_call"), false, false);
    {
        let mut inner = f.monitor.inner.lock().unwrap();
        inner.groups.get_mut(&f.id).unwrap().state = ResourceGroupState::Quarantined;
    }
    f.monitor.test_backdate_quarantine_retry(f.id);
    let previous = f.monitor.inner.lock().unwrap().groups[&f.id].kill_started_at;
    f.arm();
    assert_veto(f.run().unwrap_err(), false);
    assert_eq!(
        f.monitor.inner.lock().unwrap().groups[&f.id].kill_started_at,
        previous
    );
    assert!(f.monitor.quarantine_retry_due(f.id));
    f.recording.lock().unwrap().fail = None;
    assert!(f
        .monitor
        .retry_orphaned_quarantine(f.id, process(10, 0).identity)
        .is_ok());
    assert_eq!(f.state(), ResourceGroupState::Terminated);
}

#[test]
fn issue_3530_veto_after_effects_partial_retry() {
    let f = Fixture::new(Some("api_return"), false, false);
    f.arm();
    assert_veto(f.run().unwrap_err(), true);
    assert!(!f.monitor.quarantine_retry_due(f.id));
    f.monitor.test_backdate_quarantine_retry(f.id);
    assert!(f.monitor.quarantine_retry_due(f.id));
    f.recording.lock().unwrap().fail = None;
    f.monitor
        .retry_orphaned_quarantine(f.id, process(10, 0).identity)
        .unwrap();
    assert_eq!(f.state(), ResourceGroupState::Terminated);
}

#[test]
fn issue_3530_stale_generation() {
    let f = Fixture::new(Some("header"), false, false);
    f.arm();
    f.monitor
        .inner
        .lock()
        .unwrap()
        .groups
        .get_mut(&f.id)
        .unwrap()
        .generation += 1;
    assert!(f.run().is_ok());
    assert!(f.recording.lock().unwrap().events.is_empty());
    assert_eq!(f.backend.calls.lock().unwrap().len(), 2);
}

#[test]
fn issue_3530_implicit_and_bulk_not_armed() {
    for reason in [
        ResourceKillReason::SpawnRollback,
        ResourceKillReason::AppShutdown,
    ] {
        let f = Fixture::new(Some("header"), false, false);
        f.arm();
        f.monitor.kill_group_diagnostic(f.id, reason).unwrap();
        assert!(f.recording.lock().unwrap().events.is_empty());
    }
    let f = Fixture::new(Some("header"), false, false);
    f.arm();
    f.monitor
        .kill_all_owned_groups(ResourceKillReason::AppShutdown);
    assert!(f.recording.lock().unwrap().events.is_empty());
}

#[test]
fn issue_3530_veto_accounting_failure_stays_typed() {
    for poison in [false, true] {
        let f = Fixture::new(Some("selection"), false, false);
        let monitor = f.monitor.clone();
        let id = f.id;
        f.recording.lock().unwrap().on_fail = Some(Box::new(move || {
            if poison {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = monitor.inner.lock().unwrap();
                    panic!("injected poisoned accounting lock");
                }));
            } else {
                let mut inner = monitor.inner.lock().unwrap();
                let group = inner.groups.get_mut(&id).unwrap();
                group.generation += 1;
                group.state = ResourceGroupState::Running;
            }
        }));
        f.arm();
        match f.run().unwrap_err() {
            CleanupFailure::EvidenceVeto {
                accounting_state,
                accounting_error,
                effects_started,
                ..
            } => {
                assert_eq!(accounting_state, "unknown");
                assert!(accounting_error.is_some());
                assert!(!effects_started);
            }
            other @ CleanupFailure::Ordinary(_) => panic!("veto was degraded: {other:?}"),
        }
        assert!(f.backend.calls.lock().unwrap().is_empty());
        if !poison {
            assert_eq!(f.state(), ResourceGroupState::Running);
        }
    }
}

#[test]
fn issue_3530_ordinary_errors_preserve_behavior() {
    let f = Fixture::new(None, false, true);
    f.arm();
    let result = f.run().unwrap();
    assert_eq!(result.state, ResourceGroupState::Quarantined);
    assert!(result.message.contains("ordinary termination failure"));
    assert!(!result.message.contains("evidence_gap"));
    assert_eq!(f.backend.calls.lock().unwrap().len(), 2);
    let monitor = f.monitor.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = monitor.inner.lock().unwrap();
        panic!("ordinary lock failure");
    }));
    assert!(matches!(f.run(), Err(CleanupFailure::Ordinary(_))));
}

#[test]
fn issue_3530_open_failure_restores_state() {
    let f = Fixture::new(None, false, false);
    f.arm();
    std::fs::remove_dir(f._directory.path().join("pretermination-diagnostics")).unwrap();
    match f.run().unwrap_err() {
        CleanupFailure::EvidenceVeto {
            stage,
            effects_started,
            ..
        } => {
            assert_eq!(stage, "open");
            assert!(!effects_started);
        }
        other @ CleanupFailure::Ordinary(_) => panic!("expected open veto: {other:?}"),
    }
    assert_eq!(f.state(), ResourceGroupState::Running);
    assert!(!f.monitor.quarantine_retry_due(f.id));
    assert!(f.backend.calls.lock().unwrap().is_empty());
    assert_eq!(f.monitor.active_agent_groups(), 1);
}
