use super::*;
use std::process::{Command, Stdio};

fn entry(pid: u32, parent_pid: u32, exe: &str) -> ProcessEntry {
    ProcessEntry {
        pid,
        parent_pid,
        exe_name: exe.to_string(),
    }
}

fn identity(pid: u32, creation_time_100ns: u64) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        creation_time_100ns,
    }
}

fn no_memory(_pid: u32) -> ProcessMemory {
    ProcessMemory::default()
}

#[test]
fn matching_root_creation_time_observes_full_subtree() {
    let entries = HashMap::from([
        (1000, entry(1000, 4, "agent.exe")),
        (1001, entry(1001, 1000, "child.exe")),
    ]);
    let identities = HashMap::from([(1000, identity(1000, 111)), (1001, identity(1001, 222))]);

    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        no_memory,
    );

    assert!(
        tree.errors.is_empty(),
        "unexpected errors: {:?}",
        tree.errors
    );
    assert_eq!(tree.processes.len(), 2);
    assert_eq!(tree.processes[0].identity, identity(1000, 111));
    assert_eq!(tree.processes[0].depth, 0);
    assert_eq!(tree.processes[1].identity, identity(1001, 222));
    assert_eq!(tree.processes[1].depth, 1);
}

#[test]
fn mismatched_root_creation_time_drops_subtree() {
    // pid 1000 was registered with creation time 111, but a different
    // process (creation time 222) now occupies the pid and has spawned a
    // child. Observing the original identity must drop the whole subtree and
    // report it like a missing root, so the foreign process is never adopted
    // or later terminated.
    let entries = HashMap::from([
        (1000, entry(1000, 4, "foreign.exe")),
        (1001, entry(1001, 1000, "foreign-child.exe")),
    ]);
    let identities = HashMap::from([(1000, identity(1000, 222)), (1001, identity(1001, 333))]);

    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        no_memory,
    );

    assert!(
        tree.processes.is_empty(),
        "recycled-root subtree must be dropped, got {:?}",
        tree.processes
    );
    assert_eq!(
        tree.errors,
        vec!["root pid 1000 was not in process snapshot".to_string()]
    );
}

#[test]
fn unreadable_root_drops_subtree_and_protects_readable_child() {
    // #543 (follow-up to #516) - pid 1000 was registered with creation time
    // 111, but the live process now occupying the pid is UNREADABLE: its
    // identity resolves to None (creation_time 0). The #516 mismatch check
    // alone skipped the guard for this case, so a foreign but READABLE child
    // hanging off the unreadable root was still walked and marked
    // kill_allowed = true. The widened guard must drop the whole subtree
    // exactly like a recycled root, so the readable child is never observed
    // and can never be terminated.
    let entries = HashMap::from([
        (1000, entry(1000, 4, "foreign-unreadable.exe")),
        (1001, entry(1001, 1000, "foreign-child.exe")),
    ]);
    // Root identity is unreadable (absent from the map -> None); only the
    // readable child resolves to a real identity.
    let identities = HashMap::from([(1001, identity(1001, 222))]);

    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        no_memory,
    );

    assert!(
        tree.processes.is_empty(),
        "unverifiable-root subtree must be dropped, got {:?}",
        tree.processes
    );
    assert!(
        !tree.processes.iter().any(|p| p.kill_allowed),
        "no descendant under an unverifiable root may be kill_allowed"
    );
    assert_eq!(
        tree.errors,
        vec!["root pid 1000 was not in process snapshot".to_string()]
    );
}

#[test]
fn missing_root_reports_not_in_snapshot() {
    let entries: HashMap<u32, ProcessEntry> = HashMap::new();
    let identities: HashMap<u32, ProcessIdentity> = HashMap::new();

    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        no_memory,
    );

    assert!(tree.processes.is_empty());
    assert_eq!(
        tree.errors,
        vec!["root pid 1000 was not in process snapshot".to_string()]
    );
}

#[test]
fn resolves_identity_only_for_subtree_pids() {
    // Two unrelated processes (2000/2001) live in the snapshot but are not in
    // the root's subtree. The identity resolver must never be invoked for them;
    // that is exactly the ~498-PID cost #564 removes.
    let entries = HashMap::from([
        (1000, entry(1000, 4, "agent.exe")),
        (1001, entry(1001, 1000, "child.exe")),
        (2000, entry(2000, 4, "unrelated.exe")),
        (2001, entry(2001, 2000, "unrelated-child.exe")),
    ]);
    let identities = HashMap::from([
        (1000, identity(1000, 111)),
        (1001, identity(1001, 222)),
        (2000, identity(2000, 333)),
        (2001, identity(2001, 444)),
    ]);
    let mut probed: Vec<u32> = Vec::new();
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| {
            probed.push(pid);
            identities.get(&pid).copied()
        },
        no_memory,
    );

    assert!(
        tree.errors.is_empty(),
        "unexpected errors: {:?}",
        tree.errors
    );
    assert_eq!(tree.processes.len(), 2);
    assert!(probed.contains(&1000) && probed.contains(&1001));
    assert!(
        !probed.contains(&2000) && !probed.contains(&2001),
        "identity resolver must not touch processes outside the subtree, probed={probed:?}"
    );
}

/// #2443 bounded fixture: panics on the first `memory_for` call beyond the
/// snapshot size, so a looping walk fails fast instead of growing without limit.
fn bounded_memory(limit: usize) -> impl FnMut(u32) -> ProcessMemory {
    let mut calls: u32 = 0;
    move |_pid| {
        calls += 1;
        assert!(calls <= limit as u32, "walk revisited a pid (#2443)");
        ProcessMemory::default()
    }
}

#[test]
fn self_parent_root_is_observed_once() {
    let entries = HashMap::from([
        (1000, entry(1000, 1000, "agent.exe")),
        (1001, entry(1001, 1000, "child.exe")),
    ]);
    let identities = HashMap::from([(1000, identity(1000, 111)), (1001, identity(1001, 222))]);
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        bounded_memory(entries.len()),
    );

    let pids: Vec<u32> = tree.processes.iter().map(|p| p.identity.pid).collect();
    let depths: Vec<u32> = tree.processes.iter().map(|p| p.depth).collect();
    assert_eq!(pids, vec![1000, 1001]);
    assert_eq!(depths, vec![0, 1]);
    assert!(tree.processes.iter().all(|p| p.kill_allowed));
    assert!(
        tree.errors.is_empty(),
        "unexpected errors: {:?}",
        tree.errors
    );
}

#[test]
fn cycle_through_root_terminates_with_each_pid_once() {
    let entries = HashMap::from([
        (1000, entry(1000, 1002, "agent.exe")),
        (1001, entry(1001, 1000, "child.exe")),
        (1002, entry(1002, 1001, "grandchild.exe")),
    ]);
    let identities = HashMap::from([
        (1000, identity(1000, 111)),
        (1001, identity(1001, 222)),
        (1002, identity(1002, 333)),
    ]);
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        bounded_memory(entries.len()),
    );

    let pids: Vec<u32> = tree.processes.iter().map(|p| p.identity.pid).collect();
    let depths: Vec<u32> = tree.processes.iter().map(|p| p.depth).collect();
    assert_eq!(pids, vec![1000, 1001, 1002]);
    assert_eq!(depths, vec![0, 1, 2]);
    assert!(tree.processes.iter().all(|p| p.kill_allowed));
    assert!(
        tree.errors.is_empty(),
        "unexpected errors: {:?}",
        tree.errors
    );
}

#[test]
fn unreachable_cycle_is_not_walked() {
    let entries = HashMap::from([
        (1000, entry(1000, 4, "agent.exe")),
        (1001, entry(1001, 1000, "child.exe")),
        (2001, entry(2001, 2002, "cycle-a.exe")),
        (2002, entry(2002, 2001, "cycle-b.exe")),
    ]);
    let identities = HashMap::from([
        (1000, identity(1000, 111)),
        (1001, identity(1001, 222)),
        (2001, identity(2001, 333)),
        (2002, identity(2002, 444)),
    ]);
    let mut probed: Vec<u32> = Vec::new();
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| {
            probed.push(pid);
            identities.get(&pid).copied()
        },
        no_memory,
    );

    let pids: Vec<u32> = tree.processes.iter().map(|p| p.identity.pid).collect();
    assert_eq!(pids, vec![1000, 1001]);
    assert!(
        !probed.contains(&2001) && !probed.contains(&2002),
        "identity resolver must not touch the unreachable cycle, probed={probed:?}"
    );
}

#[test]
fn cycle_with_recycled_root_still_drops_subtree() {
    let entries = HashMap::from([
        (1000, entry(1000, 1002, "foreign.exe")),
        (1001, entry(1001, 1000, "child.exe")),
        (1002, entry(1002, 1001, "grandchild.exe")),
    ]);
    let identities = HashMap::from([
        (1000, identity(1000, 222)),
        (1001, identity(1001, 333)),
        (1002, identity(1002, 444)),
    ]);
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        no_memory,
    );

    assert!(tree.processes.is_empty());
    assert_eq!(
        tree.errors,
        vec!["root pid 1000 was not in process snapshot".to_string()]
    );
}

#[test]
fn long_cycle_emits_each_pid_at_most_once() {
    let mut entries = HashMap::from([(1000, entry(1000, 1005, "agent.exe"))]);
    let mut identities = HashMap::from([(1000, identity(1000, 111))]);
    for pid in 1001..=1005 {
        entries.insert(pid, entry(pid, pid - 1, "child.exe"));
        identities.insert(pid, identity(pid, u64::from(pid)));
    }
    let tree = build_observed_tree(
        &entries,
        identity(1000, 111),
        |pid| identities.get(&pid).copied(),
        bounded_memory(entries.len()),
    );

    let unique: HashSet<u32> = tree.processes.iter().map(|p| p.identity.pid).collect();
    let depths: Vec<u32> = tree.processes.iter().map(|p| p.depth).collect();
    assert_eq!(tree.processes.len(), 6);
    assert_eq!(unique.len(), 6);
    assert_eq!(depths, vec![0, 1, 2, 3, 4, 5]);
}

/// #1438 - real-process guard for the corpse-aware probe, following the
/// `pty/job.rs` real-process precedent. The `Child` is deliberately kept in
/// scope and un-waited: its open process handle keeps the terminated process
/// queryable, which is the exact production condition (the PTY layer holds the
/// same kind of handle) that made `Ok(Terminated)` unreachable.
#[test]
fn terminate_verified_reports_terminated_then_already_gone_for_real_process() {
    let mut child = Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ping child");
    let pid = child.id();

    let backend = PlatformProcessTreeBackend::new();
    let live = backend
        .observe_identity(pid)
        .expect("observe live child")
        .expect("a live child must resolve to an identity");
    assert!(
        observe_identity(pid).expect("probe live child").is_some(),
        "negative control: a live process must be observable to the kill path"
    );

    let observed = ObservedProcess {
        identity: live,
        parent_pid: None,
        parent_identity: None,
        exe_name: "ping.exe".to_string(),
        depth: 0,
        private_bytes: None,
        working_set_bytes: None,
        cpu_percent: None,
        kill_allowed: true,
    };

    assert_eq!(
        backend
            .terminate_verified(&observed)
            .expect("first terminate must verify the exit"),
        TerminateOutcome::Terminated
    );

    // The `Child` handle is still held here: the exact production condition.
    assert!(
        observe_identity(pid).expect("probe corpse").is_none(),
        "a corpse must not be observable to the kill path"
    );
    assert!(
        observe_identity_queryable(pid)
            .expect("resolve corpse")
            .is_some(),
        "a corpse must stay queryable to the tree resolver (#1438 G1)"
    );

    assert_eq!(
        backend
            .terminate_verified(&observed)
            .expect("retry must not error"),
        TerminateOutcome::AlreadyGone,
        "a watchdog retry over a dead target must converge without TerminateProcess"
    );

    child.wait().expect("reap the child");
}

use crate::cleanup_diagnostics::operation::EvidenceWriter;
use crate::cleanup_diagnostics::schema::{EvidenceError, EvidenceEvent, EvidenceRecord};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct DiagnosticRecording {
    events: Vec<EvidenceEvent>,
    synced: usize,
    fail: Option<&'static str>,
}
struct DiagnosticWriter(Arc<Mutex<DiagnosticRecording>>);
impl EvidenceWriter for DiagnosticWriter {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        let stage = match record.event {
            EvidenceEvent::PreCall { .. } => "pre_call",
            EvidenceEvent::ApiReturn { .. } => "api_return",
            _ => "other",
        };
        if state.fail == Some(stage) {
            return Err(EvidenceError::new("fake", "injected failure"));
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

struct FakeTerminationCalls {
    alive: Cell<bool>,
    current: schema::CurrentIdentity,
    calls: RefCell<Vec<&'static str>>,
    recording: Arc<Mutex<DiagnosticRecording>>,
    ok: bool,
}
impl TerminationCalls for FakeTerminationCalls {
    type Handle = u32;
    fn observe(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        self.calls.borrow_mut().push("observe");
        Ok(self.alive.get().then(|| identity(pid, 100)))
    }
    fn open(&self, pid: u32) -> Result<u32, ResourceError> {
        self.calls.borrow_mut().push("open");
        Ok(pid + 1000)
    }
    fn current(&self, handle: &u32, pid: u32) -> schema::CurrentIdentity {
        assert_eq!(*handle, pid + 1000);
        self.calls.borrow_mut().push("final_handle");
        self.current.clone()
    }
    fn terminate(&self, handle: &u32) -> schema::RawApiResult {
        assert!(*handle >= 1000);
        let state = self.recording.lock().unwrap();
        assert!(matches!(
            state.events[state.synced - 1],
            EvidenceEvent::PreCall { .. }
        ));
        self.calls.borrow_mut().push("terminate");
        self.alive.set(false);
        schema::RawApiResult {
            returned: Some(self.ok),
            os_error: (!self.ok).then_some(5),
            detail: "fake BOOL".into(),
        }
    }
    fn wait(&self, handle: &u32, _pid: u32) -> (schema::ExitObservation, Option<String>) {
        assert!(*handle >= 1000);
        self.calls.borrow_mut().push("wait");
        (schema::ExitObservation::Exited { code: Some(1) }, None)
    }
}

fn diagnostic_process(pid: u32) -> ObservedProcess {
    ObservedProcess {
        identity: identity(pid, 100),
        parent_pid: None,
        parent_identity: None,
        exe_name: "retained".into(),
        depth: 0,
        private_bytes: None,
        working_set_bytes: None,
        cpu_percent: None,
        kill_allowed: true,
    }
}

fn diagnostic_row(index: usize, pid: u32) -> schema::SelectionRow {
    schema::SelectionRow {
        index,
        expected: schema::ProcessIdentity {
            pid,
            start_time: 100,
        },
        retained: None,
        fresh: None,
        provenance: schema::Provenance::Fresh,
        rule: "fake selection".into(),
        current: schema::CurrentIdentity::unknown(),
        matches_app: None,
        matches_other_roots: Vec::new(),
    }
}

fn diagnostic_fixture() -> (Operation, FakeTerminationCalls) {
    let recording = Arc::new(Mutex::new(DiagnosticRecording::default()));
    let operation = Operation::new(
        uuid::Uuid::new_v4(),
        Box::new(DiagnosticWriter(Arc::clone(&recording))),
    )
    .unwrap();
    let calls = FakeTerminationCalls {
        alive: Cell::new(true),
        current: schema::CurrentIdentity::unknown(),
        calls: RefCell::new(Vec::new()),
        recording,
        ok: true,
    };
    (operation, calls)
}

#[test]
fn issue_3530_final_handle_differs() {
    let (operation, mut calls) = diagnostic_fixture();
    let row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    calls.current.identity = Some(schema::ProcessIdentity {
        pid: 42,
        start_time: 200,
    });
    calls.current.executable = Some("current-final-handle".into());
    assert_eq!(
        terminate_with_calls(&diagnostic_process(42), Some((&row, &operation)), &calls).unwrap(),
        TerminateOutcome::Terminated
    );
    let state = calls.recording.lock().unwrap();
    let final_handle = state
        .events
        .iter()
        .find_map(|event| match event {
            EvidenceEvent::PreCall { final_handle, .. } => Some(final_handle),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_handle.status, schema::IdentityStatus::Reused);
    assert_eq!(
        final_handle.executable.as_deref(),
        Some("current-final-handle")
    );
    assert_eq!(
        *calls.calls.borrow(),
        [
            "observe",
            "open",
            "final_handle",
            "terminate",
            "wait",
            "observe"
        ]
    );
}

#[test]
fn issue_3530_precall_without_api() {
    let (operation, calls) = diagnostic_fixture();
    let row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    let _ticket = operation
        .prepare(
            &row,
            schema::CurrentIdentity::unknown(),
            schema::CurrentIdentity::unknown(),
        )
        .unwrap();
    // Simulated interruption after durable preparation and before entering the native port.
    drop(operation);
    let state = calls.recording.lock().unwrap();
    assert!(state
        .events
        .iter()
        .any(|event| matches!(event, EvidenceEvent::PreCall { .. })));
    assert!(!state
        .events
        .iter()
        .any(|event| matches!(event, EvidenceEvent::ApiReturn { .. })));
    assert!(calls.calls.borrow().is_empty());
}

#[test]
fn issue_3530_windows_diagnostic_two_rows() {
    let (operation, calls) = diagnostic_fixture();
    let rows = [diagnostic_row(0, 42), diagnostic_row(1, 43)];
    operation.selection(&rows).unwrap();
    for row in &rows {
        calls.alive.set(true);
        terminate_with_calls(
            &diagnostic_process(row.expected.pid),
            Some((row, &operation)),
            &calls,
        )
        .unwrap();
    }
    let state = calls.recording.lock().unwrap();
    let indexes: Vec<_> = state
        .events
        .iter()
        .filter_map(|event| match event {
            EvidenceEvent::PreCall { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indexes, [0, 1]);
}

#[test]
fn issue_3530_windows_diagnostic_altered_row() {
    let (operation, calls) = diagnostic_fixture();
    let mut row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    row.rule = "altered".into();
    assert!(matches!(
        terminate_with_calls(&diagnostic_process(42), Some((&row, &operation)), &calls),
        Err(DiagnosticTerminationError::Evidence(_))
    ));
    assert!(!calls.calls.borrow().contains(&"terminate"));
}

#[test]
fn issue_3530_windows_diagnostic_other_process() {
    let (operation, calls) = diagnostic_fixture();
    let row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    assert!(
        matches!(terminate_with_calls(&diagnostic_process(43), Some((&row, &operation)), &calls),
        Err(DiagnosticTerminationError::Evidence(error)) if error.stage == "pre_call")
    );
    assert!(calls.calls.borrow().is_empty());
}

#[test]
fn issue_3530_windows_diagnostic_skipped_indices() {
    let (operation, calls) = diagnostic_fixture();
    let rows = [diagnostic_row(0, 42), diagnostic_row(1, 43)];
    operation.selection(&rows).unwrap();
    terminate_with_calls(
        &diagnostic_process(43),
        Some((&rows[1], &operation)),
        &calls,
    )
    .unwrap();
    let state = calls.recording.lock().unwrap();
    assert!(state
        .events
        .iter()
        .any(|event| matches!(event, EvidenceEvent::PreCall { index: 1, .. })));
    assert!(!state
        .events
        .iter()
        .any(|event| matches!(event, EvidenceEvent::PreCall { index: 0, .. })));
}

#[test]
fn issue_3530_windows_diagnostic_row_failure_after_n() {
    let (operation, calls) = diagnostic_fixture();
    let rows = [diagnostic_row(0, 42), diagnostic_row(1, 43)];
    operation.selection(&rows).unwrap();
    terminate_with_calls(
        &diagnostic_process(42),
        Some((&rows[0], &operation)),
        &calls,
    )
    .unwrap();
    calls.alive.set(true);
    let mut row = rows[1].clone();
    row.rule = "altered".into();
    match terminate_with_calls(&diagnostic_process(43), Some((&row, &operation)), &calls)
        .unwrap_err()
    {
        DiagnosticTerminationError::Evidence(error) => assert!(error.effects_started),
        other => panic!("expected evidence failure: {other:?}"),
    }
    assert_eq!(
        calls
            .calls
            .borrow()
            .iter()
            .filter(|call| **call == "terminate")
            .count(),
        1
    );
}

#[test]
fn issue_3530_windows_fail_before_api() {
    let (operation, calls) = diagnostic_fixture();
    let row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    calls.recording.lock().unwrap().fail = Some("pre_call");
    assert!(
        matches!(terminate_with_calls(&diagnostic_process(42), Some((&row, &operation)), &calls),
        Err(DiagnosticTerminationError::Evidence(error)) if !error.effects_started)
    );
    assert!(!calls.calls.borrow().contains(&"terminate"));
}

#[test]
fn issue_3530_windows_api_return_preserves_bool_error() {
    let (operation, mut calls) = diagnostic_fixture();
    calls.ok = false;
    let row = diagnostic_row(0, 42);
    operation.selection(std::slice::from_ref(&row)).unwrap();
    calls.recording.lock().unwrap().fail = Some("api_return");
    match terminate_with_calls(&diagnostic_process(42), Some((&row, &operation)), &calls)
        .unwrap_err()
    {
        DiagnosticTerminationError::Evidence(error) => {
            assert!(error.effects_started);
            let raw = error.known_api_return.unwrap();
            assert_eq!(raw.returned, Some(false));
            assert_eq!(raw.os_error, Some(5));
        }
        other => panic!("expected evidence failure: {other:?}"),
    }
    assert!(!calls.calls.borrow().contains(&"wait"));
}
