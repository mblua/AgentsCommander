use super::{execute_resource_kill_transaction, register_running, test_limits, ResourcePtyBackend};
use crate::cleanup_diagnostics::operation::EvidenceWriter;
use crate::cleanup_diagnostics::schema::{Binding, EvidenceError, EvidenceEvent, EvidenceRecord};
use crate::cleanup_diagnostics::store::{ArmStatus, Clock, Diagnostics};
use crate::pty::backend::{PtyBackend, SessionBackendKind};
use crate::pty::manager::PtyManager;
use crate::resource_monitor::registry::{ProcessTreeBackend, ResourceError};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceGroupState,
    TerminateOutcome,
};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::selection::{SelectionTransaction, TrustedResourceIntent};
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

struct FixedClock;
impl Clock for FixedClock {
    fn utc(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap()
    }
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
}

#[derive(Default)]
struct Recording {
    events: Vec<EvidenceEvent>,
    veto: bool,
    replace: Option<Box<dyn FnOnce() + Send>>,
}
struct Writer(Arc<Mutex<Recording>>);
impl EvidenceWriter for Writer {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError> {
        let mut state = self.0.lock().unwrap();
        state.events.push(record.event.clone());
        if state.veto && matches!(record.event, EvidenceEvent::SelectionComplete { .. }) {
            if let Some(replace) = state.replace.take() {
                replace();
            }
            return Err(EvidenceError::new("selection", "caller fixture veto"));
        }
        Ok(())
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
    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        Ok(ObservedProcessTree {
            processes: if self.alive.load(Ordering::SeqCst) {
                vec![ObservedProcess {
                    identity: self.root,
                    parent_pid: None,
                    parent_identity: None,
                    exe_name: "fake-agent".into(),
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
            // Model an ordinary error followed by readonly confirmation of external exit.
            self.alive.store(false, Ordering::SeqCst);
            return Err("ordinary caller fixture error".into());
        }
        self.alive.store(false, Ordering::SeqCst);
        Ok(TerminateOutcome::Terminated)
    }
    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    app: tauri::App<tauri::test::MockRuntime>,
    id: Uuid,
    monitor: Arc<ResourceMonitorState>,
    process: Arc<Backend>,
    pty: Arc<ResourcePtyBackend>,
    recording: Arc<Mutex<Recording>>,
    expect_unknown: bool,
}
impl Fixture {
    async fn new(veto: bool, ordinary: bool, replace: bool) -> Self {
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
        let process = Arc::new(Backend {
            root: ProcessIdentity {
                pid: 7530,
                creation_time_100ns: 3530,
            },
            alive: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            samples: AtomicUsize::new(0),
            ordinary,
        });
        let monitor = Arc::new(ResourceMonitorState::with_backend_spawn_and_diagnostics(
            process.clone(),
            Arc::new(crate::pty::spawn_diagnostics::SpawnRegistry::new()),
            diagnostics.clone(),
        ));
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let id = manager
            .read()
            .await
            .create_session(
                "shell".into(),
                Vec::new(),
                directory.path().to_string_lossy().into_owned(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap()
            .id;
        Self::arm(&monitor, &diagnostics, &recording, &process, id, replace);
        let pty = Arc::new(ResourcePtyBackend::default());
        pty.set_live(id);
        let pty_manager = Arc::new(Mutex::new(PtyManager::new_for_test(pty.clone())));
        pty_manager
            .lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
        let telegram = Arc::new(tokio::sync::Mutex::new(
            crate::telegram::manager::TelegramBridgeManager::with_captures(
                Arc::new(Mutex::new(std::collections::HashMap::new())),
                Arc::new(crate::capture::registry::CaptureRegistry::new()),
            ),
        ));
        let app = tauri::test::mock_builder()
            .manage(manager)
            .manage(pty_manager)
            .manage(monitor.clone())
            .manage(DetachedSessionsState::default())
            .manage(telegram)
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        Self {
            _directory: directory,
            app,
            id,
            monitor,
            process,
            pty,
            recording,
            expect_unknown: replace,
        }
    }
    fn arm(
        monitor: &Arc<ResourceMonitorState>,
        diagnostics: &Diagnostics,
        recording: &Mutex<Recording>,
        process: &Backend,
        id: Uuid,
        replace: bool,
    ) {
        register_running(monitor, id, process.root);
        assert!(monitor.has_registered_group(id));
        assert_eq!(monitor.active_agent_groups(), 1);
        let binding = Binding {
            run_id: diagnostics.run_id(),
            session_id: id,
            root: Some(crate::cleanup_diagnostics::schema::ProcessIdentity {
                pid: process.root.pid,
                start_time: process.root.creation_time_100ns,
            }),
            registration_generation: 1,
        };
        let armed = diagnostics
            .publish_arm_metadata(diagnostics.reserve().unwrap(), binding.clone())
            .unwrap();
        let ArmStatus::Armed { operation_id } = armed else {
            panic!("first arm")
        };
        assert!(matches!(diagnostics.arm_status_metadata(&binding),
            Some(ArmStatus::AlreadyArmed { operation_id: id }) if id == operation_id));
        if replace {
            let monitor = Arc::clone(monitor);
            let root = process.root;
            recording.lock().unwrap().replace = Some(Box::new(move || {
                register_running(&monitor, id, root);
            }));
        }
    }
    async fn run(&self, route: u8, veto: bool) {
        let transaction = SelectionTransaction::for_test(self.app.handle().clone());
        match route {
            0 => {
                let result = execute_resource_kill_transaction(
                    &transaction,
                    self.id,
                    TrustedResourceIntent::User,
                )
                .await
                .unwrap();
                assert_eq!(result.finalized, !veto);
                if veto {
                    assert!(result.message.contains("evidence_gap"));
                    if self.expect_unknown {
                        assert!(result.message.contains("unknown"));
                    }
                }
                assert_eq!(self.pty.terminate_count.load(Ordering::SeqCst), 1);
            }
            1 => {
                let result = crate::commands::session::destroy::teardown_legacy_destroy_runtime(
                    &transaction,
                    self.id,
                    &crate::session::selection::DestructionSource::ManualClose,
                )
                .await;
                assert_eq!(result.is_err(), veto);
                if self.expect_unknown {
                    assert!(result.unwrap_err().contains("unknown"));
                }
            }
            2 => {
                let result = crate::commands::session::restart::teardown_old_for_restart(
                    &transaction,
                    self.id,
                    false,
                )
                .await;
                if veto {
                    let error = result.err().expect("veto must stop replacement");
                    assert!(!error.old_lost);
                    assert!(error.message.contains("evidence_gap"));
                    if self.expect_unknown {
                        assert!(error.message.contains("unknown"));
                    }
                } else {
                    assert!(result.unwrap_or_else(|e| panic!("{}", e.message)));
                }
            }
            _ => unreachable!(),
        }
        assert_eq!(self.pty.kill_count.load(Ordering::SeqCst) == 0, veto);
        assert_eq!(self.pty.has_session(self.id), veto);
        assert_eq!(self.process.calls.load(Ordering::SeqCst) == 0, veto);
    }
}

#[tokio::test]
async fn issue_3530_resource_veto_no_poll_or_finalize_after_job() {
    let f = Fixture::new(true, false, false).await;
    f.run(0, true).await;
    let samples = f.process.samples.load(Ordering::SeqCst);
    assert_eq!(
        f.monitor.snapshot(test_limits()).groups[0].state,
        ResourceGroupState::Running
    );
    assert_eq!(f.process.samples.load(Ordering::SeqCst), samples + 1);
    assert_eq!(
        f.recording
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|event| matches!(event, EvidenceEvent::SelectionComplete { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn issue_3530_destroy_veto_no_pty() {
    Fixture::new(true, false, false).await.run(1, true).await;
}

#[tokio::test]
async fn issue_3530_restart_veto_no_replacement() {
    Fixture::new(true, false, false).await.run(2, true).await;
}

#[tokio::test]
async fn issue_3530_veto_then_accounting_generation_failure_three_routes() {
    for route in 0..3 {
        let f = Fixture::new(true, false, true).await;
        f.run(route, true).await;
        let snapshot = f.monitor.snapshot(test_limits());
        assert_eq!(snapshot.groups[0].state, ResourceGroupState::Running);
        assert!(snapshot.groups[0].last_error.is_none());
        // Registry owns the typed unknown/error assertion; no private timestamp read.
    }
}

#[tokio::test]
async fn issue_3530_ordinary_errors_preserve_flow() {
    for route in 0..3 {
        for ordinary in [false, true] {
            let f = Fixture::new(false, ordinary, false).await;
            f.run(route, false).await;
        }
    }
}
