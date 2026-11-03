use super::{complete_isolated, run_isolated, ProtectedFixture};
use crate::cleanup_diagnostics::operation::EvidenceWriter;
use crate::cleanup_diagnostics::schema::{self, EvidenceError, EvidenceEvent, EvidenceRecord};
use crate::cleanup_diagnostics::store::{ArmStatus, Clock, Diagnostics};
use crate::commands::session::destroy::reconcile_response_closes;
use crate::pty::backend::PtyBackend;
use crate::resource_monitor::registry::{
    ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceGroupState,
    ResourceLaunchMetadata, ResourceLimits, TerminateOutcome,
};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::selection::response_close::ResponseEffectOwner;
use crate::session::session::SessionStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

const PREFIX: &str = "commands::session::selection_executor::tests::protected_carrier::cleanup::";
struct FixedClock;
impl Clock for FixedClock {
    fn utc(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap()
    }
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
}
struct Gate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}
#[derive(Default)]
struct Recording {
    events: Vec<EvidenceEvent>,
    gate: Option<Gate>,
    veto: bool,
}
struct Writer(Arc<Mutex<Recording>>);
impl EvidenceWriter for Writer {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        let (gate, veto) = {
            let mut state = self.0.lock().unwrap();
            state.events.push(record.event.clone());
            if !matches!(record.event, EvidenceEvent::SelectionComplete { .. }) {
                return Ok(());
            }
            (state.gate.take(), state.veto)
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            // Sender drop releases this worker even when a test assertion unwinds.
            let _ = gate.release.recv_timeout(Duration::from_secs(30));
        }
        if veto {
            Err(EvidenceError::new("selection", "response fixture veto"))
        } else {
            Ok(())
        }
    }
    fn flush_sync(&mut self) -> Result<(), EvidenceError> {
        Ok(())
    }
    fn complete(&mut self) -> Result<(), EvidenceError> {
        Ok(())
    }
}
struct Backend {
    root: ProcessIdentity,
    alive: AtomicBool,
    calls: AtomicUsize,
    samples: AtomicUsize,
    ordinary: bool,
}
impl ProcessTreeBackend for Backend {
    fn supports_process_tree_enforcement(&self) -> bool {
        true
    }
    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        Ok((pid == self.root.pid && self.alive.load(Ordering::SeqCst)).then_some(self.root))
    }
    fn observe_tree(&self, _: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        Ok(ObservedProcessTree {
            processes: if self.alive.load(Ordering::SeqCst) {
                vec![ObservedProcess {
                    identity: self.root,
                    parent_pid: None,
                    parent_identity: None,
                    exe_name: "response-fake".into(),
                    depth: 0,
                    private_bytes: None,
                    working_set_bytes: None,
                    cpu_percent: None,
                    kill_allowed: true,
                }]
            } else {
                Vec::new()
            },
            errors: Vec::new(),
        })
    }
    fn terminate_verified(&self, _: &ObservedProcess) -> Result<TerminateOutcome, ResourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.ordinary {
            self.alive.store(false, Ordering::SeqCst);
            return Err("ordinary response error".into());
        }
        self.alive.store(false, Ordering::SeqCst);
        Ok(TerminateOutcome::Terminated)
    }
    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}
fn limits() -> ResourceLimits {
    ResourceLimits {
        monitor_enabled: true,
        max_concurrent_agent_processes: 4,
        group_warn_private_bytes: u64::MAX,
        group_kill_private_bytes: u64::MAX,
        process_kill_private_bytes: u64::MAX,
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    diagnostics: Arc<Diagnostics>,
    monitor: Arc<ResourceMonitorState>,
    backend: Arc<Backend>,
    recording: Arc<Mutex<Recording>>,
}
impl Fixture {
    fn new(veto: bool, ordinary: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let recording = Arc::new(Mutex::new(Recording {
            veto,
            ..Recording::default()
        }));
        let output = recording.clone();
        let diagnostics = Arc::new(
            Diagnostics::with_test_ports(
                directory.path(),
                Arc::new(FixedClock),
                Arc::new(move || Box::new(Writer(output.clone()))),
            )
            .unwrap(),
        );
        let backend = Arc::new(Backend {
            root: ProcessIdentity {
                pid: 83530,
                creation_time_100ns: 3530,
            },
            alive: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            samples: AtomicUsize::new(0),
            ordinary,
        });
        let monitor = Arc::new(ResourceMonitorState::with_backend_spawn_and_diagnostics(
            backend.clone(),
            Arc::new(crate::pty::spawn_diagnostics::SpawnRegistry::new()),
            diagnostics.clone(),
        ));
        Self {
            _directory: directory,
            diagnostics,
            monitor,
            backend,
            recording,
        }
    }
    fn register(&self, id: Uuid) {
        let permit = self
            .monitor
            .try_reserve_agent_slot(limits())
            .unwrap()
            .unwrap();
        let mut registration = ResourceLaunchRegistration::new(
            (*self.monitor).clone(),
            permit,
            ResourceLaunchMetadata {
                session_id: id,
                name: "response".into(),
                agent_id: None,
                agent_label: None,
                workgroup: None,
                agent: None,
                project: None,
            },
        );
        assert_eq!(
            registration
                .register_root_pid(self.backend.root.pid)
                .unwrap(),
            Some(self.backend.root)
        );
        assert!(self.monitor.has_registered_group(id));
        assert_eq!(self.monitor.active_agent_groups(), 1);
    }
    fn binding(&self, id: Uuid) -> schema::Binding {
        schema::Binding {
            run_id: self.diagnostics.run_id(),
            session_id: id,
            root: Some(schema::ProcessIdentity {
                pid: self.backend.root.pid,
                start_time: self.backend.root.creation_time_100ns,
            }),
            registration_generation: 1,
        }
    }
    fn arm(&self, id: Uuid) {
        self.register(id);
        let binding = self.binding(id);
        let ArmStatus::Armed { operation_id } = self
            .diagnostics
            .publish_arm_metadata(self.diagnostics.reserve().unwrap(), binding.clone())
            .unwrap()
        else {
            panic!("first arm");
        };
        assert!(matches!(self.diagnostics.arm_status_metadata(&binding),
            Some(ArmStatus::AlreadyArmed { operation_id: id }) if id == operation_id));
    }
    fn gate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, reached) = tokio::sync::oneshot::channel();
        let (release, receiver) = std::sync::mpsc::channel();
        self.recording.lock().unwrap().gate = Some(Gate {
            entered,
            release: receiver,
        });
        (reached, release)
    }
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.backend.samples.load(Ordering::SeqCst),
            self.backend.calls.load(Ordering::SeqCst),
            self.recording.lock().unwrap().events.len(),
        )
    }
}
fn owners(f: &ProtectedFixture) -> Vec<Arc<ResponseEffectOwner>> {
    f.native
        .app
        .state::<crate::app_state::AppOwners>()
        .response_effects
        .snapshot()
        .unwrap()
}
fn disk() -> Vec<u8> {
    let directory = crate::config::config_dir().unwrap();
    let rows = crate::config::sessions_persistence::load_sessions_raw_from_dir_for_test(&directory);
    assert!(!rows.is_empty());
    std::fs::read(directory.join("sessions.json")).unwrap()
}
async fn veto_case(retain: bool, generation: bool) {
    let d = Fixture::new(true, false);
    let f = std::rc::Rc::new(ProtectedFixture::new_with_monitor(false, d.monitor.clone()).await);
    let n = &f.native;
    d.arm(n.source);
    n.app
        .state::<crate::app_state::DetachedSessionsState>()
        .lock()
        .unwrap()
        .insert(n.source);
    if retain {
        n.app
            .state::<crate::app_state::AppOwners>()
            .response_effects
            .force_next_settlement_for_test(n.source);
    }
    let before = n.manager.read().await.get_session(n.source).await.unwrap();
    let (entered, release) = d.gate();
    let producer_fixture = f.clone();
    let producer = tokio::task::spawn_local(async move {
        producer_fixture.retire().await;
    });
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    let owner = owners(&f)
        .pop()
        .expect("real core retained owner before group");
    assert_eq!(owner.registration().member.session_id, n.source);
    assert!(owner.ticket().committed_binding().member == owner.registration().member);
    assert!(d
        .diagnostics
        .arm_status_metadata(&d.binding(n.source))
        .is_none());
    let persisted = disk();
    assert!(
        crate::config::sessions_persistence::load_sessions_raw_from_dir_for_test(
            &crate::config::config_dir().unwrap()
        )
        .iter()
        .any(|row| row.id.as_deref() == Some(n.source.to_string().as_str()))
    );
    if generation {
        d.register(n.source);
    }
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(40), producer)
        .await
        .unwrap()
        .unwrap();
    assert_veto(&f, &d, &owner, &before, &persisted, retain).await;
    n.coordinator.close_and_join().await;
}

async fn assert_veto(
    f: &ProtectedFixture,
    d: &Fixture,
    owner: &Arc<ResponseEffectOwner>,
    before: &crate::session::session::Session,
    persisted: &[u8],
    retain: bool,
) {
    let n = &f.native;
    assert!(owner.kill_failed());
    assert!(owner.terminal());
    assert_eq!(owner.diagnostic_terminal(), Some(true));
    assert_eq!(n.backend.kill_count(n.source), 0);
    assert!(n.backend.has_session(n.source));
    let after = n.manager.read().await.get_session(n.source).await.unwrap();
    assert_eq!(
        (
            after.id,
            after.token,
            after.backend_kind,
            after.working_directory,
            after.status
        ),
        (
            before.id,
            before.token,
            before.backend_kind,
            before.working_directory.clone(),
            before.status.clone()
        )
    );
    assert!(n
        .app
        .state::<crate::app_state::DetachedSessionsState>()
        .lock()
        .unwrap()
        .contains(&n.source));
    assert_eq!(disk(), persisted);
    let snapshot = d.monitor.snapshot(limits());
    assert_eq!(snapshot.groups[0].state, ResourceGroupState::Running);
    assert!(snapshot.groups[0].last_error.is_none());
    assert_eq!(snapshot.groups[0].root_identity, d.backend.root);
    // Gen2 timestamp None is a source invariant: registry initialization + pass.owns rejection.
    let frozen = d.counts();
    assert_eq!(frozen.1, 0);
    assert_eq!(
        d.recording
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|event| matches!(event, EvidenceEvent::OperationBegin))
            .count(),
        1
    );
    for _ in 0..2 {
        reconcile_response_closes(n.app.handle()).await;
        assert_eq!(d.counts(), frozen);
        assert_eq!(n.backend.kill_count(n.source), 0);
        assert!(owner.kill_failed() && owner.terminal());
        assert_eq!(disk(), persisted);
        if retain {
            assert!(owner.forced_settlement_for_test());
            assert!(Arc::ptr_eq(&owners(f)[0], owner));
            assert!(crate::session::selection::response_close_registration(n.source).is_some());
        } else {
            assert!(owners(&f).is_empty());
            assert!(crate::session::selection::response_close_registration(n.source).is_none());
        }
    }
}

async fn positive_case(ordinary: bool) {
    let d = Fixture::new(false, ordinary);
    let f = std::rc::Rc::new(ProtectedFixture::new_with_monitor(false, d.monitor.clone()).await);
    d.arm(f.native.source);
    let (entered, release) = d.gate();
    let producer_fixture = f.clone();
    let producer = tokio::task::spawn_local(async move {
        producer_fixture.retire().await;
    });
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    let owner = owners(&f)
        .pop()
        .expect("positive core reached diagnostic group");
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(40), producer)
        .await
        .unwrap()
        .unwrap();
    assert!(!owner.kill_failed());
    assert!(owner.terminal());
    assert!(d.backend.calls.load(Ordering::SeqCst) > 0);
    assert!(f.native.backend.kill_count(f.native.source) > 0);
    assert!(!f.native.backend.has_session(f.native.source));
    assert!(owners(&f).is_empty());
    assert!(crate::session::selection::response_close_registration(f.native.source).is_none());
    assert_eq!(
        f.native
            .manager
            .read()
            .await
            .get_session(f.native.source)
            .await
            .unwrap()
            .status,
        SessionStatus::Exited(0)
    );
    f.native.coordinator.close_and_join().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3530_response_veto_reopens_live() {
    let name = format!("{PREFIX}issue_3530_response_veto_reopens_live");
    run_isolated(&name, async {
        tokio::task::LocalSet::new()
            .run_until(veto_case(false, false))
            .await;
        complete_isolated(&name);
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3530_response_veto_reconcile_never_calls() {
    let name = format!("{PREFIX}issue_3530_response_veto_reconcile_never_calls");
    run_isolated(&name, async {
        tokio::task::LocalSet::new()
            .run_until(veto_case(true, false))
            .await;
        complete_isolated(&name);
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3530_response_veto_accounting_generation_failure() {
    let name = format!("{PREFIX}issue_3530_response_veto_accounting_generation_failure");
    run_isolated(&name, async {
        tokio::task::LocalSet::new()
            .run_until(veto_case(true, true))
            .await;
        complete_isolated(&name);
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3530_response_ordinary_and_success_preserve_flow() {
    let name = format!("{PREFIX}issue_3530_response_ordinary_and_success_preserve_flow");
    run_isolated(&name, async {
        tokio::task::LocalSet::new()
            .run_until(async {
                for ordinary in [false, true] {
                    positive_case(ordinary).await;
                }
            })
            .await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3530_waiter_cancel_does_not_forge_result() {
    use crate::resource_monitor::registry::termination::CleanupFailure;
    use futures::FutureExt;
    let name = format!("{PREFIX}issue_3530_waiter_cancel_does_not_forge_result");
    run_isolated(&name, async {
        let first = Fixture::new(true, false);
        let f = ProtectedFixture::new_with_monitor(false, first.monitor.clone()).await;
        first.arm(f.native.source);
        f.native
            .app
            .state::<crate::app_state::AppOwners>()
            .response_effects
            .force_next_settlement_for_test(f.native.source);
        f.retire().await;
        let original = owners(&f).pop().unwrap();
        // Separate owner polling test, not a second claim about the core execution.
        let owner = Arc::new(ResponseEffectOwner::new(
            original.ticket().clone(),
            original.registration().clone(),
            original.backend().clone(),
        ));
        let d = Fixture::new(true, false);
        d.arm(f.native.source);
        let Some(ArmStatus::AlreadyArmed { operation_id }) = d
            .diagnostics
            .arm_status_metadata(&d.binding(f.native.source))
        else {
            panic!("original armed operation");
        };
        let (entered, release) = d.gate();
        let monitor = d.monitor.clone();
        let id = f.native.source;
        owner
            .start_group_diagnostic(|| {
                tokio::task::spawn_blocking(move || {
                    monitor.kill_group_diagnostic(
                        id,
                        crate::resource_monitor::types::ResourceKillReason::SessionDestroy,
                    )
                })
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        assert!(
            std::future::poll_fn(|cx| owner.poll_group_diagnostic(cx, id))
                .now_or_never()
                .is_none()
        );
        // now_or_never dropped only the pending waiter; the worker remains owned.
        release.send(()).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| owner.poll_group_diagnostic(cx, id)),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(CleanupFailure::EvidenceVeto {
            operation_id: actual, effects_started: false, ..
        }) if actual == operation_id));
        assert_eq!(d.backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.native.backend.kill_count(id), 0);
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}
