use super::*;
use crate::pty::backend::SessionBackendKind;
use crate::pty::container_paths::CLAUDE_CONFIG_DIR_KEY;
use crate::pty::container_runtime::{ContainerCleanupReport, RETAINED_OWNER_REPORT_CAPACITY};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
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

struct PermanentlyBlockedStopRuntime {
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
            .unwrap_or_else(|error| error.into_inner())
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

impl ContainerRuntime for PermanentlyBlockedStopRuntime {
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
        loop {
            std::thread::park();
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

mod global_sweep;
mod lifecycle;
mod shutdown_work;
mod transport;
