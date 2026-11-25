use super::shutdown_work::{ContainerShutdownWorkContext, ContainerShutdownWorkProvenance};
use super::start_request::{build_start_request_with_settings, sanitized_child_env};
use super::*;
use crate::pty::backend::SessionBackendKind;
use crate::pty::container_paths::ContainerPathMap;
use crate::pty::container_paths::CLAUDE_CONFIG_DIR_KEY;
use crate::pty::container_runtime::{ContainerCleanupReport, RETAINED_OWNER_REPORT_CAPACITY};
use crate::pty::container_runtime::{
    ContainerDiagnostics, ContainerRuntimeControl, ContainerStartRequest,
    RetainedContainerOwnerContext,
};
use crate::pty::container_tokens::ContainerApiToken;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct RecordingRuntime {
    started: AtomicUsize,
    stopped: Arc<Mutex<Vec<Uuid>>>,
}

impl RecordingRuntime {
    fn start_count(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn stopped(&self) -> Vec<Uuid> {
        self.stopped.lock().unwrap().clone()
    }
}

impl ContainerRuntime for RecordingRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        _control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        Ok(ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        handle: &ContainerRuntimeHandle,
        _timeout: Duration,
        _control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        self.stopped.lock().unwrap().push(handle.session_id);
        Ok(())
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        Ok(ContainerCleanupReport::default())
    }
}

struct GatedStartRuntime {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    stopped: Arc<Mutex<Vec<Uuid>>>,
}

struct SequencedStopRuntime {
    outcomes: Mutex<VecDeque<Result<(), &'static str>>>,
    stop_calls: AtomicUsize,
    active_deadline_seen: AtomicBool,
}

struct RetainedContextRuntime {
    contexts: Vec<RetainedContainerOwnerContext>,
}

impl ContainerRuntime for GatedStartRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        _control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        self.release
            .lock()
            .unwrap()
            .recv()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        handle: &ContainerRuntimeHandle,
        _timeout: Duration,
        _control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        self.stopped.lock().unwrap().push(handle.session_id);
        Ok(())
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        Ok(ContainerCleanupReport::default())
    }
}

impl ContainerRuntime for SequencedStopRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        _control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        Ok(ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        _handle: &ContainerRuntimeHandle,
        _timeout: Duration,
        control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
        if control
            .remaining()
            .is_some_and(|remaining| !remaining.is_zero())
        {
            self.active_deadline_seen.store(true, Ordering::SeqCst);
        }
        match self
            .outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Ok(()))
        {
            Ok(()) => Ok(()),
            Err(error) => Err(AppError::Other(error.to_string())),
        }
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        Ok(ContainerCleanupReport::default())
    }
}

impl ContainerRuntime for RetainedContextRuntime {
    fn start(
        &self,
        request: ContainerStartRequest,
        _control: &ContainerRuntimeControl,
    ) -> Result<ContainerRuntimeHandle, AppError> {
        Ok(ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        _handle: &ContainerRuntimeHandle,
        _timeout: Duration,
        _control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        Ok(())
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError> {
        Ok(ContainerCleanupReport::default())
    }

    fn retained_cleanup_contexts(&self) -> Vec<RetainedContainerOwnerContext> {
        self.contexts.clone()
    }
}

fn backend_with_tuning(
    tuning: ContainerTransportTuning,
) -> (
    ContainerTransportBackend,
    Arc<tokio::sync::RwLock<SessionManager>>,
) {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    (
        ContainerTransportBackend::with_tuning(output_senders, idle_detector, None, None, tuning),
        session_mgr,
    )
}

fn test_spec(id: Uuid, root: &str, output_target: PtyOutputTarget) -> BackendSpawnSpec {
    BackendSpawnSpec {
        id,
        agent_id: None,
        coding_agent: None,
        cmd: "container".to_string(),
        args: Vec::new(),
        resolved_agent_host_shell: None,
        cwd: root.to_string(),
        selected_cwd: None,
        cols: 120,
        rows: 30,
        container_image: None,
        configured_env: Vec::new(),
        env_remove_keys: Vec::new(),
        env_unset: Vec::new(),
        extra_env: Vec::new(),
        idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
        output_target,
        resource_registration: None,
        logical_resource_slot: None,
        container_credential: None,
        container_repo_mounts: Vec::new(),
        launch_witness: Default::default(),
    }
}

async fn pending_backend(id: Uuid, root: &str) -> (ContainerTransportBackend, String) {
    let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
    backend
        .spawn(test_spec(id, root, PtyOutputTarget::noop()))
        .await
        .expect("spawn pending");
    let ticket = backend
        .last_issued_ticket_for_test(id)
        .expect("issued test ticket");
    (backend, ticket)
}

fn attach(
    backend: &ContainerTransportBackend,
    id: Uuid,
    root: &str,
    ticket: &str,
) -> mpsc::Receiver<HostToBridgeFrame> {
    backend
        .consume_ticket(id, root, ticket)
        .expect("consume ticket");
    let (tx, rx) = mpsc::channel(8);
    backend.complete_hello(id, root, tx).expect("hello");
    rx
}

fn api_enabled_settings() -> crate::config::settings::AppSettings {
    crate::config::settings::AppSettings {
        api_server_enabled: true,
        api_server_bind: "0.0.0.0".to_string(),
        api_server_port: 8765,
        ..crate::config::settings::AppSettings::default()
    }
}

/// The bridge's repaint, delivered as soon as the Resize is published, finds the
/// resize mark already taken: it touches silence but keeps the team stamp. A failed queue
/// reservation takes no mark and keeps its original error.
async fn attached_with_idle(
    id: Uuid,
    root: &str,
) -> (
    ContainerTransportBackend,
    Arc<IdleDetector>,
    mpsc::Receiver<HostToBridgeFrame>,
) {
    let idle = IdleDetector::new(|_| {}, |_| {});
    let senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let tuning = ContainerTransportTuning::default();
    let backend =
        ContainerTransportBackend::with_tuning(senders, Arc::clone(&idle), None, None, tuning);
    let spec = test_spec(id, root, PtyOutputTarget::noop());
    backend.spawn(spec).await.expect("spawn pending");
    let ticket = backend.last_issued_ticket_for_test(id).expect("ticket");
    let rx = attach(&backend, id, root, &ticket);
    (backend, idle, rx)
}

#[tokio::test]
async fn issue_2158_container_resize_marks_before_publish() {
    let id = Uuid::new_v4();
    let root = "C:/repo/.ac/wg-1/__agent_dev";
    let (backend, idle, rx) = attached_with_idle(id, root).await;
    let rx = Mutex::new(rx);
    // Established: alive 31 min, silent 30 min, first chunk long outside the window.
    let (silence, alive) = (Duration::from_secs(1800), Duration::from_secs(1860));
    idle.set_auto_close_ages_for_test(id, silence, alive);
    let team_before = idle
        .team_silence_age_at(id, Instant::now())
        .expect("eligible");
    let delivered = Arc::new(AtomicBool::new(false));
    let hook_delivered = Arc::clone(&delivered);
    backend.set_resize_published_hook(Box::new(move |backend| {
        let frame = rx.lock().unwrap().try_recv().expect("published Resize");
        assert!(matches!(
            frame,
            HostToBridgeFrame::Text(HostToBridgeTextFrame::Resize { cols: 100, .. })
        ));
        backend
            .handle_bridge_output(id, vec![b'x'; 4096])
            .expect("repaint");
        hook_delivered.store(true, Ordering::SeqCst);
    }));
    backend.resize(id, 100, 40).expect("resize");
    assert!(delivered.load(Ordering::SeqCst));
    assert!(
        idle.silence_age(id).unwrap() < Duration::from_secs(60),
        "repaint touched silence"
    );
    let team_after = idle
        .team_silence_age_at(id, Instant::now())
        .expect("eligible");
    assert!(
        team_after >= team_before,
        "the repaint must not move the team stamp"
    );
    assert_eq!(backend.get_pty_size(id), Some((40, 100)));
    assert_eq!(backend.resize_marks_for_test(), 1);

    // Full and Closed: no mark, the original errors and cleanup.
    for full in [true, false] {
        let id = Uuid::new_v4();
        let (backend, idle, rx) = attached_with_idle(id, root).await;
        let (removed_tx, removed_rx) = std::sync::mpsc::channel();
        backend.set_route_remover(Arc::new(move |session_id, _| {
            let _ = removed_tx.send(session_id);
            Ok(())
        }));
        if full {
            for _ in 0..8 {
                backend.send_ping(id).expect("fill queue");
            }
        } else {
            drop(rx);
        }
        let error = backend.resize(id, 90, 30).expect_err("queue failure");
        if full {
            let full = "container transport outbound queue full";
            assert!(matches!(error, AppError::PtyError(ref t) if t == full));
        } else {
            assert!(matches!(error, AppError::SessionNotFound(ref t) if t == &id.to_string()));
        }
        assert!(!backend.has_session(id), "queue failure keeps its cleanup");
        assert_eq!(
            backend.resize_marks_for_test(),
            0,
            "the prepublish mark never ran"
        );
        let removed = removed_rx.recv_timeout(Duration::from_secs(1));
        assert_eq!(removed.ok(), Some(id), "route reconciliation");
        assert_eq!(backend.get_pty_size(id), None, "dimensions unchanged");
        let readiness = idle.purge_readiness(&[id]);
        assert!(
            readiness.iter().all(|r| r.last_resize_age.is_none()),
            "no mark"
        );
    }
}

/// Backend over `runtime` with no broadcaster, lifecycle sender or token manager.
fn backend_with_runtime(runtime: Arc<dyn ContainerRuntime>) -> ContainerTransportBackend {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    ContainerTransportBackend::with_runtime(
        output_senders,
        idle_detector,
        None,
        None,
        runtime,
        None,
    )
}

/// Registers an active runtime handle for `session_id` on container `container-<id>`.
fn insert_runtime_handle(
    backend: &ContainerTransportBackend,
    session_id: Uuid,
) -> mpsc::Receiver<HostToBridgeFrame> {
    backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
        session_id,
        container_id: format!("container-{session_id}"),
    })
}

/// The sweep dropped the transport state but kept the runtime cleanup owner
/// for `session_id`, and nothing was stopped yet.
fn assert_runtime_owner_retained(
    backend: &ContainerTransportBackend,
    runtime: &RecordingRuntime,
    session_id: Uuid,
) {
    assert!(!backend.contains_transport_state_for_test(session_id));
    assert_eq!(
        backend.retained_runtime_cleanup_sessions_for_test(),
        vec![session_id]
    );
    assert!(runtime.stopped().is_empty());
}

fn production_route_remover_fixture(
    runtime: Arc<RecordingRuntime>,
) -> (
    Arc<ContainerTransportBackend>,
    Arc<Mutex<PtyManager>>,
    Uuid,
    mpsc::Receiver<HostToBridgeFrame>,
) {
    let backend = Arc::new(backend_with_runtime(runtime));
    let local_backend: Arc<dyn PtyBackend> = backend.clone();
    let pty_manager = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
        local_backend,
        Arc::clone(&backend),
    )));
    let session_id = Uuid::new_v4();
    let receiver = insert_runtime_handle(&backend, session_id);
    pty_manager
        .lock()
        .unwrap()
        .record_route(session_id, SessionBackendKind::ContainerTransport);
    crate::install_container_route_remover(&pty_manager);
    (backend, pty_manager, session_id, receiver)
}

/// Logical clock of one sweep invocation: `now` is an atomic load and only the conductor
/// moves it. A wait on a gated site acks the site and parks until that site is stepped;
/// other sites return at once. `rescue` opens every gate past any deadline.
struct SweepDriver {
    origin: Instant,
    offset: AtomicU64,
    gates: Mutex<(HashMap<&'static str, u64>, bool)>,
    changed: std::sync::Condvar,
    acks: std::sync::mpsc::Sender<&'static str>,
    ack_rx: Mutex<(std::sync::mpsc::Receiver<&'static str>, Vec<&'static str>)>,
    waits: AtomicUsize,
    probes: Mutex<Vec<SweepProbe>>,
}

const WATCHDOG: Duration = Duration::from_secs(5);

impl SweepDriver {
    /// Starts at an origin already past on the physical clock, gating `sites`.
    fn new(sites: &[&'static str]) -> Arc<Self> {
        let (acks, ack_rx) = std::sync::mpsc::channel();
        let origin = Instant::now() - Duration::from_secs(120);
        Arc::new(Self {
            origin,
            offset: AtomicU64::new(0),
            gates: Mutex::new((sites.iter().map(|site| (*site, 0)).collect(), false)),
            changed: std::sync::Condvar::new(),
            acks,
            ack_rx: Mutex::new((ack_rx, Vec::new())),
            waits: AtomicUsize::new(0),
            probes: Mutex::new(Vec::new()),
        })
    }

    fn now(&self) -> Instant {
        self.origin + Duration::from_nanos(self.offset.load(Ordering::SeqCst))
    }

    fn set(&self, at: Instant) {
        let offset = at.duration_since(self.origin).as_nanos();
        self.offset
            .store(offset.try_into().unwrap(), Ordering::SeqCst);
    }

    fn wait(&self, site: &'static str) {
        self.waits.fetch_add(1, Ordering::SeqCst);
        let mut gates = self.gates.lock().unwrap();
        let Some(generation) = gates.0.get(site).copied().filter(|_| !gates.1) else {
            return;
        };
        let _ = self.acks.send(site);
        let limit = Instant::now() + WATCHDOG;
        while gates.0.get(site) == Some(&generation) && !gates.1 && Instant::now() < limit {
            gates = self.changed.wait_timeout(gates, WATCHDOG).unwrap().0;
        }
    }

    fn step(&self, site: &'static str) {
        *self
            .gates
            .lock()
            .unwrap()
            .0
            .get_mut(site)
            .expect("gated site") += 1;
        self.changed.notify_all();
    }

    /// Opens every gate without moving the clock.
    fn open(&self) {
        self.gates.lock().unwrap().1 = true;
        self.changed.notify_all();
    }

    fn rescue(&self) {
        self.set(self.now() + Duration::from_secs(3600));
        self.open();
    }

    /// Next ack of `site`; acks of other sites stay queued. Panics after the watchdog.
    fn ack(&self, site: &'static str) {
        let (receiver, backlog) = &mut *self.ack_rx.lock().unwrap();
        let limit = Instant::now() + WATCHDOG;
        if let Some(index) = backlog.iter().position(|seen| *seen == site) {
            backlog.remove(index);
            return;
        }
        while Instant::now() < limit {
            match receiver.recv_timeout(WATCHDOG) {
                Ok(seen) if seen == site => return,
                Ok(other) => backlog.push(other),
                Err(_) => break,
            }
        }
        panic!("no wait acknowledged at site {site}");
    }

    /// Fails if `site` acks a new wait before `done` holds.
    fn settle(&self, site: &'static str, mut done: impl FnMut() -> bool) {
        let (receiver, backlog) = &mut *self.ack_rx.lock().unwrap();
        backlog.extend(receiver.try_iter());
        assert!(!backlog.contains(&site), "{site} waited again");
        let limit = Instant::now() + WATCHDOG;
        while !done() {
            assert_ne!(receiver.try_recv().ok(), Some(site), "{site} waited again");
            assert!(Instant::now() < limit, "{site} did not settle");
            std::thread::yield_now();
        }
    }

    fn probes(&self, mark: SweepProbe) -> usize {
        self.probes
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| **seen == mark)
            .count()
    }

    fn clock(self: &Arc<Self>) -> ContainerSweepClock {
        let (now, wait, probe) = (Arc::clone(self), Arc::clone(self), Arc::clone(self));
        ContainerSweepClock::for_test(
            Arc::new(move || now.now()),
            Arc::new(move |site, _| wait.wait(site)),
            Some(Arc::new(move |mark| {
                probe.probes.lock().unwrap().push(mark);
                if mark == SweepProbe::EpochOpened {
                    probe.wait("epochOpened");
                }
            })),
        )
    }

    /// Runs the real sweep with this clock on its own thread.
    fn spawn_sweep(
        self: &Arc<Self>,
        backend: &Arc<ContainerTransportBackend>,
        budget: Duration,
    ) -> (
        std::thread::JoinHandle<()>,
        std::sync::mpsc::Receiver<ContainerShutdownReport>,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (backend, clock) = (Arc::clone(backend), self.clock());
        let caller = std::thread::spawn(move || {
            let report = backend.stop_all_started_containers_blocking_with_clock(budget, &clock);
            let _ = sender.send(report);
        });
        (caller, receiver)
    }
}

/// Opens every gate when the test returns or unwinds, before any join.
struct Rescue(Arc<SweepDriver>);

impl Drop for Rescue {
    fn drop(&mut self) {
        self.0.rescue();
    }
}

/// Rows 7/12/13: validates the worker's idle ack before dispatch, waits for `entered(D)`,
/// re-evaluates once at `D - EPSILON` with the caller pending, then takes the report at D.
fn sweep_at_deadline(
    driver: &Arc<SweepDriver>,
    backend: &Arc<ContainerTransportBackend>,
    entered: impl FnOnce(Instant),
) -> ContainerShutdownReport {
    let (idle_sender, idle) = std::sync::mpsc::channel();
    backend
        .shutdown_work
        .set_idle_observer_for_test(idle_sender);
    let start = driver.now();
    let (caller, report) = driver.spawn_sweep(backend, EPSILON);
    driver.ack("epochOpened");
    assert_live_idle(&idle, start);
    driver.step("epochOpened");
    entered(start + EPSILON);
    driver.ack("drainPoll");
    driver.step("drainPoll");
    driver.ack("drainPoll");
    assert!(report.try_recv().is_err(), "the caller is pending before D");
    driver.set(start + EPSILON);
    driver.step("drainPoll");
    let mut at_deadline = None;
    driver.settle("drainPoll", || {
        at_deadline = report.try_recv().ok();
        at_deadline.is_some()
    });
    caller.join().expect("join the sweep caller");
    at_deadline.expect("report at D")
}

/// The first idle decision of the real worker at `D - EPSILON`: alive, not terminating,
/// `remaining` exactly `EPSILON`.
fn assert_live_idle(idle: &std::sync::mpsc::Receiver<SweepIdleAck>, at: Instant) {
    let ack = idle.recv_timeout(WATCHDOG).expect("worker idle ack");
    assert_eq!(
        ack,
        SweepIdleAck {
            global_sweep: true,
            observed: at,
            live: true,
            terminating: false,
            producers: 0,
            remaining: Some(EPSILON),
        }
    );
}

const EPSILON: Duration = Duration::from_secs(1);

mod global_sweep;
mod lifecycle;
mod shutdown_work;
mod transport;
