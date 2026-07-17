use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::sessions_persistence::persist_current_state_result;
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::manager::PtyManager;
use crate::session::manager::{
    CommitDecision, CommitResult, LifecycleMutations, ManagerAggregateSnapshot,
    PendingCreateBinding, SessionManager,
};
use crate::session::session::{SessionInfo, SessionStatus};
use crate::session::warnings::{emit_session_warning, SessionWarning};
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;

const COORDINATOR_QUEUE_CAPACITY: usize = 64;
const COORDINATOR_ADMISSION_CAPACITY: usize = 65;
const CREATE_TICKET_CAPACITY: usize = 16;
pub const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;
const SHUTDOWN_FINALIZATION_RESERVE_MAX: Duration = Duration::from_millis(500);

tokio::task_local! {
    static IN_SELECTION_WORKER: ();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionSource {
    InitialHydration,
    SessionCreated,
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart,
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor,
    BackgroundCleanup,
    LivenessReconcile,
}

impl fmt::Display for SelectionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::InitialHydration => "initialHydration",
            Self::SessionCreated => "sessionCreated",
            Self::UserSwitch => "userSwitch",
            Self::ManualClose => "manualClose",
            Self::AutoClose => "autoClose",
            Self::Restart => "restart",
            Self::Restore => "restore",
            Self::Detach => "detach",
            Self::Attach => "attach",
            Self::SpawnRollback => "spawnRollback",
            Self::ResourceMonitor => "resourceMonitor",
            Self::BackgroundCleanup => "backgroundCleanup",
            Self::LivenessReconcile => "livenessReconcile",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    None,
    Live,
    Dormant,
}

impl fmt::Display for SelectionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Live => f.write_str("live"),
            Self::Dormant => f.write_str("dormant"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSelection {
    epoch: String,
    source: SelectionSource,
    user_initiated: bool,
    revision: u64,
    mode: SelectionMode,
    id: Option<String>,
    status: Option<SessionStatus>,
    has_pty: bool,
    detached: bool,
    displayable: bool,
}

impl SessionSelection {
    pub(super) fn initial(epoch: Uuid) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: SelectionSource::InitialHydration,
            user_initiated: false,
            revision: 0,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(super) fn none(epoch: Uuid, revision: u64, cause: SelectionCause) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(super) fn live(epoch: Uuid, revision: u64, cause: SelectionCause, id: Uuid) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Live,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Active),
            has_pty: true,
            detached: false,
            displayable: true,
        }
    }

    pub(super) fn dormant(
        epoch: Uuid,
        revision: u64,
        cause: SelectionCause,
        id: Uuid,
        exit_code: i32,
        has_pty: bool,
    ) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Dormant,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Exited(exit_code)),
            has_pty,
            detached: false,
            displayable: false,
        }
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    pub fn source(&self) -> SelectionSource {
        self.source
    }

    pub fn user_initiated(&self) -> bool {
        self.user_initiated
    }

    pub fn id(&self) -> Option<Uuid> {
        self.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
    }

    pub fn status(&self) -> Option<&SessionStatus> {
        self.status.as_ref()
    }

    pub fn has_pty(&self) -> bool {
        self.has_pty
    }

    pub fn detached(&self) -> bool {
        self.detached
    }

    pub fn displayable(&self) -> bool {
        self.displayable
    }

    #[cfg(test)]
    pub(crate) fn live_for_test(id: Uuid) -> Self {
        Self::live(Uuid::new_v4(), 1, SelectionCause::UserSwitch, id)
    }

    #[cfg(test)]
    pub(crate) fn dormant_for_test(id: Uuid, exit_code: i32) -> Self {
        Self::dormant(
            Uuid::new_v4(),
            1,
            SelectionCause::Restore,
            id,
            exit_code,
            false,
        )
    }

    #[cfg(test)]
    pub(crate) fn none_for_test() -> Self {
        Self::none(Uuid::new_v4(), 1, SelectionCause::AutoClose)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedCreateIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedRestartIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedResourceIntent {
    User,
    Watchdog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionCause {
    SessionCreated(TrustedCreateIntent),
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart(TrustedRestartIntent),
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor(TrustedResourceIntent),
    BackgroundCleanup,
    LivenessReconcile,
}

impl SelectionCause {
    pub(super) fn source(self) -> SelectionSource {
        match self {
            Self::SessionCreated(_) => SelectionSource::SessionCreated,
            Self::UserSwitch => SelectionSource::UserSwitch,
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::Restart(_) => SelectionSource::Restart,
            Self::Restore => SelectionSource::Restore,
            Self::Detach => SelectionSource::Detach,
            Self::Attach => SelectionSource::Attach,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::ResourceMonitor(_) => SelectionSource::ResourceMonitor,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
            Self::LivenessReconcile => SelectionSource::LivenessReconcile,
        }
    }

    pub(super) fn user_initiated(self) -> bool {
        match self {
            Self::SessionCreated(TrustedCreateIntent::User)
            | Self::Restart(TrustedRestartIntent::User)
            | Self::ResourceMonitor(TrustedResourceIntent::User)
            | Self::UserSwitch
            | Self::ManualClose
            | Self::Detach
            | Self::Attach => true,
            Self::SessionCreated(TrustedCreateIntent::Background)
            | Self::Restart(TrustedRestartIntent::Background)
            | Self::ResourceMonitor(TrustedResourceIntent::Watchdog)
            | Self::AutoClose
            | Self::Restore
            | Self::SpawnRollback
            | Self::BackgroundCleanup
            | Self::LivenessReconcile => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSelectionSnapshot {
    pub session_id: Uuid,
    pub has_pty: bool,
    pub detached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveRuntimeWitness {
    pub(super) session_id: Uuid,
    pub(super) has_pty: bool,
    pub(super) detached: bool,
}

impl LiveRuntimeWitness {
    fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (snapshot.has_pty && !snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: true,
            detached: false,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DormantRuntimeWitness {
    pub(super) session_id: Uuid,
    pub(super) has_pty: bool,
    pub(super) detached: bool,
}

impl DormantRuntimeWitness {
    fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (!snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: snapshot.has_pty,
            detached: false,
        })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCoordinatorError {
    #[error("selectionCoordinatorBusy")]
    Busy,
    #[error("selectionCoordinatorUnavailable")]
    Unavailable,
    #[error("selectionCoordinatorRecursiveSubmission")]
    RecursiveSubmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CriticalAdmissionKind {
    RouteLoss,
    WatchdogKill,
    BackgroundCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CriticalAdmissionKey {
    session_id: Uuid,
    kind: CriticalAdmissionKind,
}

struct CriticalAdmissionGuard {
    inner: Weak<CoordinatorInner>,
    key: CriticalAdmissionKey,
}

impl CriticalAdmissionGuard {
    fn new(inner: &Arc<CoordinatorInner>, key: CriticalAdmissionKey) -> Self {
        Self {
            inner: Arc::downgrade(inner),
            key,
        }
    }
}

impl Drop for CriticalAdmissionGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            remove_critical_key(&inner, self.key);
        }
    }
}

struct CriticalAdmissionReservation {
    admission: OwnedSemaphorePermit,
    slot: mpsc::OwnedPermit<CoordinatorEnvelope>,
    guard: CriticalAdmissionGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalAdmissionOutcome<T> {
    Completed(T),
    AlreadyPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinatorPhase {
    Bootstrapping = 0,
    Restoring = 1,
    Running = 2,
    Closing = 3,
}

impl CoordinatorPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Restoring,
            2 => Self::Running,
            3 => Self::Closing,
            _ => Self::Bootstrapping,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionRequest {
    UserSwitch { session_id: Uuid },
    Restore { persisted_target: Option<Uuid> },
}

#[derive(Debug, Clone)]
pub(crate) struct DormantRestoreRequest {
    pub persisted: crate::config::sessions_persistence::PersistedSession,
    pub working_directory: String,
    pub is_coordinator: bool,
    pub is_root_agent: bool,
}

impl SelectionRequest {
    pub fn user_switch(session_id: Uuid) -> Self {
        Self::UserSwitch { session_id }
    }

    pub(crate) fn restore(persisted_target: Option<Uuid>) -> Self {
        Self::Restore { persisted_target }
    }
}

#[derive(Debug)]
struct FinalizeCreateRequest {
    binding: PendingCreateBinding,
    cause: SelectionCause,
    auto_select_precondition: Option<(String, u64)>,
    warnings: Vec<SessionWarning>,
}

#[derive(Debug)]
enum CoordinatorJob {
    Transition {
        request: SelectionRequest,
        response: oneshot::Sender<Result<Option<SessionSelection>, String>>,
    },
    Snapshot {
        response: oneshot::Sender<Result<SessionSelection, String>>,
    },
    FinalizeCreate {
        request: FinalizeCreateRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RollbackCreate {
        binding: PendingCreateBinding,
    },
    RouteLoss {
        session_id: Uuid,
        exit_code: i32,
        response: oneshot::Sender<Result<(), String>>,
    },
    Destroy {
        request: crate::commands::session::DestroyRequest,
        response: oneshot::Sender<Result<crate::commands::session::DestroyOutcome, String>>,
    },
    RestartLifecycle {
        request: crate::commands::session::RestartJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RootLifecycle {
        request: crate::commands::session::RootJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    ResourceKill {
        session_id: Uuid,
        intent: TrustedResourceIntent,
        response: oneshot::Sender<Result<crate::resource_monitor::ResourceKillResult, String>>,
    },
    Detach {
        session_id: Uuid,
        geometry: Option<crate::config::settings::WindowGeometry>,
        suppress_selection: bool,
        response: oneshot::Sender<Result<String, String>>,
    },
    Attach {
        session_id: Uuid,
        response: oneshot::Sender<Result<(), String>>,
    },
    RestoreBarrier {
        started: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}

struct CoordinatorEnvelope {
    job: CoordinatorJob,
    _admission: OwnedSemaphorePermit,
    _create_ticket: Option<OwnedSemaphorePermit>,
    _critical_admission: Option<CriticalAdmissionGuard>,
}

struct CoordinatorInner {
    sender: mpsc::Sender<CoordinatorEnvelope>,
    receiver: Mutex<Option<mpsc::Receiver<CoordinatorEnvelope>>>,
    admission: Arc<Semaphore>,
    create_tickets: Arc<Semaphore>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    phase: AtomicU8,
    critical_keys: Mutex<HashSet<CriticalAdmissionKey>>,
    shutdown: CancellationToken,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    retained_workers: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    retained_cleanup_tasks:
        Mutex<Vec<tokio::task::JoinHandle<crate::pty::container_backend::ContainerShutdownReport>>>,
    retained_rollback_tasks: Mutex<Vec<tokio::task::JoinHandle<Vec<String>>>>,
    cleanup_pty: Mutex<Option<Weak<Mutex<PtyManager>>>>,
    cleanup_container_backend: Mutex<Option<Weak<ContainerTransportBackend>>>,
}

#[derive(Clone)]
pub struct SelectionCoordinator {
    inner: Arc<CoordinatorInner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionShutdownReport {
    pub persistence_safe: bool,
    pub retained: Vec<String>,
}

impl SelectionCoordinator {
    pub fn new(
        manager: Arc<tokio::sync::RwLock<SessionManager>>,
        shutdown: CancellationToken,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(COORDINATOR_QUEUE_CAPACITY);
        Self {
            inner: Arc::new(CoordinatorInner {
                sender,
                receiver: Mutex::new(Some(receiver)),
                admission: Arc::new(Semaphore::new(COORDINATOR_ADMISSION_CAPACITY)),
                create_tickets: Arc::new(Semaphore::new(CREATE_TICKET_CAPACITY)),
                manager,
                phase: AtomicU8::new(CoordinatorPhase::Bootstrapping as u8),
                critical_keys: Mutex::new(HashSet::new()),
                shutdown,
                worker: Mutex::new(None),
                retained_workers: Mutex::new(Vec::new()),
                retained_cleanup_tasks: Mutex::new(Vec::new()),
                retained_rollback_tasks: Mutex::new(Vec::new()),
                cleanup_pty: Mutex::new(None),
                cleanup_container_backend: Mutex::new(None),
            }),
        }
    }

    pub fn start<R: Runtime>(&self, app: AppHandle<R>) -> Result<(), SelectionCoordinatorError> {
        if let Some(pty) = app.try_state::<Arc<Mutex<PtyManager>>>() {
            let container_backend = pty
                .inner()
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?
                .container_backend();
            let mut cleanup_pty = self
                .inner
                .cleanup_pty
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_pty = Some(Arc::downgrade(pty.inner()));
            let mut cleanup_container_backend = self
                .inner
                .cleanup_container_backend
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable)?;
            *cleanup_container_backend = Some(Arc::downgrade(&container_backend));
        }
        let receiver = self
            .inner
            .receiver
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?
            .take()
            .ok_or(SelectionCoordinatorError::Unavailable)?;
        let inner = Arc::clone(&self.inner);
        let handle = tauri::async_runtime::spawn(IN_SELECTION_WORKER.scope((), async move {
            worker_loop(app, inner, receiver).await;
        }));
        let mut worker = self
            .inner
            .worker
            .lock()
            .map_err(|_| SelectionCoordinatorError::Unavailable)?;
        *worker = Some(handle);
        Ok(())
    }

    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    fn check_external_submission(&self) -> Result<(), SelectionCoordinatorError> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission);
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Running => Ok(()),
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                Err(SelectionCoordinatorError::Busy)
            }
            CoordinatorPhase::Closing => Err(SelectionCoordinatorError::Unavailable),
        }
    }

    fn try_reserve_envelope(
        &self,
    ) -> Result<
        (OwnedSemaphorePermit, mpsc::OwnedPermit<CoordinatorEnvelope>),
        SelectionCoordinatorError,
    > {
        self.check_external_submission()?;
        let admission = Arc::clone(&self.inner.admission)
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let slot = self
            .inner
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => SelectionCoordinatorError::Unavailable,
                mpsc::error::TrySendError::Full(_) => SelectionCoordinatorError::Busy,
            })?;
        Ok((admission, slot))
    }

    pub async fn transition(
        &self,
        request: SelectionRequest,
    ) -> Result<Option<SessionSelection>, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Transition { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn destroy(
        &self,
        request: crate::commands::session::DestroyRequest,
    ) -> Result<crate::commands::session::DestroyOutcome, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn restart_lifecycle(
        &self,
        request: crate::commands::session::RestartJobRequest,
    ) -> Result<SessionInfo, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RestartLifecycle { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn root_lifecycle(
        &self,
        request: crate::commands::session::RootJobRequest,
    ) -> Result<SessionInfo, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RootLifecycle { request, response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) fn reserve_auto_close(
        &self,
    ) -> Result<AutoCloseBatchTicket, SelectionCoordinatorError> {
        let (admission, slot) = self.try_reserve_envelope()?;
        Ok(AutoCloseBatchTicket {
            slot: Some(slot),
            admission: Some(admission),
        })
    }

    pub(crate) async fn resource_kill(
        &self,
        session_id: Uuid,
        intent: TrustedResourceIntent,
    ) -> Result<crate::resource_monitor::ResourceKillResult, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ResourceKill {
                session_id,
                intent,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn detach(
        &self,
        session_id: Uuid,
        geometry: Option<crate::config::settings::WindowGeometry>,
        suppress_selection: bool,
    ) -> Result<String, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Detach {
                session_id,
                geometry,
                suppress_selection,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub(crate) async fn attach(&self, session_id: Uuid) -> Result<(), String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Attach {
                session_id,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub async fn snapshot(&self) -> Result<SessionSelection, String> {
        let (admission, slot) = self
            .try_reserve_envelope()
            .map_err(|error| error.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Snapshot { response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }

    pub async fn reserve_create(
        &self,
        intent: TrustedCreateIntent,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.reserve_create_internal(intent, true).await
    }

    pub(crate) async fn reserve_suppressed_create(
        &self,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.reserve_create_internal(TrustedCreateIntent::Background, false)
            .await
    }

    async fn reserve_create_internal(
        &self,
        intent: TrustedCreateIntent,
        allow_auto_select: bool,
    ) -> Result<CreateFinalizationTicket, SelectionCoordinatorError> {
        self.check_external_submission()?;
        let create_ticket = Arc::clone(&self.inner.create_tickets)
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let admission = Arc::clone(&self.inner.admission)
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let slot = self
            .inner
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => SelectionCoordinatorError::Unavailable,
                mpsc::error::TrySendError::Full(_) => SelectionCoordinatorError::Busy,
            })?;

        let manager = self.inner.manager.read().await.clone();
        let selection = manager.selection_payload().await;
        let auto_select_precondition = (allow_auto_select
            && selection.mode() == SelectionMode::None)
            .then(|| (selection.epoch().to_string(), selection.revision()));
        Ok(CreateFinalizationTicket {
            slot: Some(slot),
            admission: Some(admission),
            create_ticket: Some(create_ticket),
            binding: None,
            cause: SelectionCause::SessionCreated(intent),
            auto_select_precondition,
            completed: false,
        })
    }

    pub async fn submit_restore_first(&self) -> Result<RestoreBarrierGuard, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        self.inner
            .phase
            .compare_exchange(
                CoordinatorPhase::Bootstrapping as u8,
                CoordinatorPhase::Restoring as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| SelectionCoordinatorError::Busy.to_string())?;
        let admission = match Arc::clone(&self.inner.admission).try_acquire_owned() {
            Ok(admission) => admission,
            Err(_) => {
                self.inner
                    .phase
                    .store(CoordinatorPhase::Bootstrapping as u8, Ordering::Release);
                return Err(SelectionCoordinatorError::Busy.to_string());
            }
        };
        let slot = match self.inner.sender.clone().try_reserve_owned() {
            Ok(slot) => slot,
            Err(_) => {
                self.inner
                    .phase
                    .store(CoordinatorPhase::Bootstrapping as u8, Ordering::Release);
                return Err(SelectionCoordinatorError::Busy.to_string());
            }
        };
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RestoreBarrier {
                started,
                release: release_rx,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        // Restore now owns FIFO position one. Normal and critical work may be
        // admitted, but the worker cannot execute it until restore releases the
        // barrier. In particular, unsolicited route loss must wait here instead
        // of being rejected after the external route has already disappeared.
        self.inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        started_rx
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?;
        Ok(RestoreBarrierGuard {
            release: Some(release),
        })
    }

    async fn reserve_critical_admission(
        &self,
        key: CriticalAdmissionKey,
    ) -> Result<Option<CriticalAdmissionReservation>, String> {
        let guard = {
            let mut keys = self
                .inner
                .critical_keys
                .lock()
                .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?;
            if !keys.insert(key) {
                return Ok(None);
            }
            CriticalAdmissionGuard::new(&self.inner, key)
        };

        let admission = tokio::select! {
            permit = Arc::clone(&self.inner.admission).acquire_owned() => {
                permit.map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
            }
            _ = self.inner.shutdown.cancelled() => {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
        };
        let slot = tokio::select! {
            permit = self.inner.sender.clone().reserve_owned() => {
                permit.map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
            }
            _ = self.inner.shutdown.cancelled() => {
                return Err(SelectionCoordinatorError::Unavailable.to_string());
            }
        };
        Ok(Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }))
    }

    async fn submit_route_loss(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::Completed(()));
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::RouteLoss,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RouteLoss {
                session_id,
                exit_code,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(()))
    }

    pub(crate) async fn watchdog_resource_kill(
        &self,
        session_id: Uuid,
    ) -> Result<CriticalAdmissionOutcome<crate::resource_monitor::ResourceKillResult>, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::WatchdogKill,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::ResourceKill {
                session_id,
                intent: TrustedResourceIntent::Watchdog,
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        let result = receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(result))
    }

    pub(crate) async fn background_destroy(
        &self,
        session_id: Uuid,
    ) -> Result<CriticalAdmissionOutcome<crate::commands::session::DestroyOutcome>, String> {
        if IN_SELECTION_WORKER.try_with(|_| ()).is_ok() {
            return Err(SelectionCoordinatorError::RecursiveSubmission.to_string());
        }
        match CoordinatorPhase::from_u8(self.inner.phase.load(Ordering::Acquire)) {
            CoordinatorPhase::Bootstrapping | CoordinatorPhase::Restoring => {
                return Err(SelectionCoordinatorError::Busy.to_string())
            }
            CoordinatorPhase::Closing => {
                return Err(SelectionCoordinatorError::Unavailable.to_string())
            }
            CoordinatorPhase::Running => {}
        }
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        }
        let key = CriticalAdmissionKey {
            session_id,
            kind: CriticalAdmissionKind::BackgroundCleanup,
        };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy {
                request: crate::commands::session::DestroyRequest {
                    ids: vec![session_id],
                    source: crate::commands::session::DestructionSource::BackgroundCleanup,
                    force_destroy_root: false,
                },
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        let result = receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(result))
    }

    #[cfg(test)]
    pub(crate) fn critical_key_registered_for_test(
        &self,
        session_id: Uuid,
        kind: CriticalAdmissionKind,
    ) -> bool {
        self.inner
            .critical_keys
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&CriticalAdmissionKey { session_id, kind })
    }

    #[cfg(test)]
    async fn critical_probe_for_test(
        &self,
        session_id: Uuid,
        kind: CriticalAdmissionKind,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        let manager = self.inner.manager.read().await.clone();
        if !manager.contains_public_or_pending(session_id).await {
            return Ok(CriticalAdmissionOutcome::Completed(()));
        }
        let key = CriticalAdmissionKey { session_id, kind };
        let Some(CriticalAdmissionReservation {
            admission,
            slot,
            guard,
        }) = self.reserve_critical_admission(key).await?
        else {
            return Ok(CriticalAdmissionOutcome::AlreadyPending);
        };
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Snapshot { response },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: Some(guard),
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())??;
        Ok(CriticalAdmissionOutcome::Completed(()))
    }

    pub fn container_lifecycle_sender(&self) -> ContainerLifecycleSender {
        ContainerLifecycleSender {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub async fn close_and_join(&self) -> SelectionShutdownReport {
        self.close_and_join_with_budget(Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS))
            .await
    }

    async fn close_and_join_with_budget(&self, budget: Duration) -> SelectionShutdownReport {
        let started_at = Instant::now();
        let deadline = started_at.checked_add(budget).unwrap_or(started_at);
        let finalization_reserve = (budget / 4).min(SHUTDOWN_FINALIZATION_RESERVE_MAX);
        let worker_deadline = deadline
            .checked_sub(finalization_reserve)
            .unwrap_or(started_at);
        self.inner
            .phase
            .store(CoordinatorPhase::Closing as u8, Ordering::Release);
        self.inner.admission.close();
        self.inner.create_tickets.close();
        self.inner.shutdown.cancel();
        let mut retained = Vec::new();
        if !begin_container_shutdown(&self.inner, deadline) {
            retained.push("reason=container-shutdown-signal state=retained".to_string());
        }

        let handle = match self.inner.worker.try_lock() {
            Ok(mut worker) => worker.take(),
            Err(error) => {
                log::error!(
                    "[selection] worker handle lock unavailable during bounded shutdown: {error}"
                );
                retained.push("reason=coordinator-worker-handle state=retained".to_string());
                None
            }
        };
        if let Some(mut handle) = handle {
            let remaining = worker_deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    log::error!("[selection] coordinator worker join failed: {error}");
                }
                Err(_) => {
                    let critical_count = self
                        .inner
                        .critical_keys
                        .lock()
                        .map(|keys| keys.len())
                        .unwrap_or_default();
                    log::error!(
                        "[selection] coordinator worker phase exhausted before shared deadline budget={:?} finalizationReserve={:?} outstandingCriticalKeys={}",
                        budget,
                        finalization_reserve,
                        critical_count
                    );
                    handle.abort();
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, &mut handle).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            log::debug!("[selection] aborted worker join result: {error}");
                        }
                        Err(_) => {
                            log::error!(
                                "[selection] aborted coordinator worker retained after absolute deadline state=retained"
                            );
                            retained
                                .push("reason=coordinator-worker-abort state=retained".to_string());
                            self.inner
                                .retained_workers
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .push(handle);
                        }
                    }
                }
            }
        }

        retained.extend(cleanup_pending_creates_after_join(&self.inner, deadline).await);
        retained
            .extend(seal_and_drain_container_shutdown_work_after_join(&self.inner, deadline).await);
        match self.inner.critical_keys.try_lock() {
            Ok(mut keys) => keys.clear(),
            Err(error) => {
                log::error!("[selection] critical key lock unavailable after worker join: {error}");
                retained.push("reason=critical-key-clear state=retained".to_string());
            }
        }
        retained.sort();
        retained.dedup();
        SelectionShutdownReport {
            persistence_safe: retained.is_empty(),
            retained,
        }
    }
}

pub struct RestoreBarrierGuard {
    release: Option<oneshot::Sender<()>>,
}

pub(crate) struct AutoCloseBatchTicket {
    slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    admission: Option<OwnedSemaphorePermit>,
}

impl AutoCloseBatchTicket {
    pub async fn finalize(
        mut self,
        ids: Vec<Uuid>,
    ) -> Result<crate::commands::session::DestroyOutcome, String> {
        let slot = self
            .slot
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let admission = self
            .admission
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy {
                request: crate::commands::session::DestroyRequest {
                    ids,
                    source: crate::commands::session::DestructionSource::AutoClose,
                    force_destroy_root: false,
                },
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }
}

impl RestoreBarrierGuard {
    pub(crate) fn transaction<R: Runtime>(&self, app: AppHandle<R>) -> SelectionTransaction<R> {
        SelectionTransaction::new(app)
    }

    fn release_worker(&mut self) {
        if let Some(release) = self.release.take() {
            if release.send(()).is_err() {
                log::warn!("[selection] restore barrier worker ended before completion signal");
            }
        }
    }

    pub fn finish(mut self) {
        self.release_worker();
    }
}

impl Drop for RestoreBarrierGuard {
    fn drop(&mut self) {
        if self.release.is_some() {
            log::warn!("[selection] restore barrier released by dropped owner");
            self.release_worker();
        }
    }
}

pub struct CreateFinalizationTicket {
    slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    admission: Option<OwnedSemaphorePermit>,
    create_ticket: Option<OwnedSemaphorePermit>,
    binding: Option<PendingCreateBinding>,
    cause: SelectionCause,
    auto_select_precondition: Option<(String, u64)>,
    completed: bool,
}

impl fmt::Debug for CreateFinalizationTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateFinalizationTicket")
            .field("binding", &self.binding)
            .field("auto_select_precondition", &self.auto_select_precondition)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

impl CreateFinalizationTicket {
    pub(super) fn bind(&mut self, session_id: Uuid) -> PendingCreateBinding {
        let binding = PendingCreateBinding::new(session_id, Uuid::new_v4());
        self.binding = Some(binding);
        binding
    }

    pub(crate) fn binding(&self) -> Option<PendingCreateBinding> {
        self.binding
    }

    pub async fn finalize(mut self, warnings: Vec<SessionWarning>) -> Result<SessionInfo, String> {
        let slot = self
            .slot
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let admission = self
            .admission
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let create_ticket = self
            .create_ticket
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let binding = self
            .binding
            .take()
            .ok_or_else(|| "create finalization ticket was not bound".to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::FinalizeCreate {
                request: FinalizeCreateRequest {
                    binding,
                    cause: self.cause,
                    auto_select_precondition: self.auto_select_precondition.take(),
                    warnings,
                },
                response,
            },
            _admission: admission,
            _create_ticket: Some(create_ticket),
            _critical_admission: None,
        }));
        self.completed = true;
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }
}

impl Drop for CreateFinalizationTicket {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let Some(binding) = self.binding.take() else {
            return;
        };
        let (Some(slot), Some(admission), Some(create_ticket)) = (
            self.slot.take(),
            self.admission.take(),
            self.create_ticket.take(),
        ) else {
            log::error!(
                "[selection] bound create ticket dropped without its reserved finalization capacity session={}",
                binding.session_id()
            );
            return;
        };
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RollbackCreate { binding },
            _admission: admission,
            _create_ticket: Some(create_ticket),
            _critical_admission: None,
        }));
        self.completed = true;
    }
}

#[derive(Clone)]
pub struct ContainerLifecycleSender {
    inner: Weak<CoordinatorInner>,
}

impl ContainerLifecycleSender {
    pub async fn route_lost(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        let inner = self
            .inner
            .upgrade()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        SelectionCoordinator { inner }
            .submit_route_loss(session_id, exit_code)
            .await
    }
}

pub(crate) struct SelectionTransaction<R: Runtime> {
    app: AppHandle<R>,
    capability: CommitCapability,
}

impl<R: Runtime> Clone for SelectionTransaction<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            capability: self.capability.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct CommitCapability {
    _private: (),
}

#[cfg(test)]
impl CommitCapability {
    pub(super) fn for_test() -> Self {
        Self { _private: () }
    }
}

impl<R: Runtime> SelectionTransaction<R> {
    fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            capability: CommitCapability { _private: () },
        }
    }

    pub(crate) fn app(&self) -> &AppHandle<R> {
        &self.app
    }

    pub(crate) async fn manager(&self) -> SessionManager {
        self.app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_pending_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<crate::session::session::SessionRepo>,
        is_coordinator: bool,
        backend_kind: crate::pty::backend::SessionBackendKind,
    ) -> Result<(crate::session::session::Session, PendingCreateBinding), String> {
        self.manager()
            .await
            .create_transaction_pending_session(
                shell,
                shell_args,
                working_directory,
                agent_id,
                agent_label,
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn finalize_inline_create(
        &self,
        binding: PendingCreateBinding,
        cause: SelectionCause,
        warnings: Vec<SessionWarning>,
    ) -> Result<SessionInfo, String> {
        execute_finalize_create(
            self,
            FinalizeCreateRequest {
                binding,
                cause,
                auto_select_precondition: None,
                warnings,
            },
        )
        .await
    }

    pub(crate) async fn rollback_inline_create(&self, binding: PendingCreateBinding) {
        execute_rollback_create(self, binding).await;
    }

    pub(crate) async fn reconcile_route_loss_inline(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<(), String> {
        execute_route_loss(self, session_id, exit_code).await
    }

    pub(crate) async fn restore_dormant_inline(
        &self,
        request: DormantRestoreRequest,
    ) -> Result<SessionInfo, String> {
        execute_restore_dormant(self, request).await
    }

    pub(crate) async fn restore_selection_inline(
        &self,
        persisted_target: Option<Uuid>,
    ) -> Result<Option<SessionSelection>, String> {
        execute_transition(self, SelectionRequest::restore(persisted_target)).await
    }

    pub(crate) async fn aggregate_snapshot(&self) -> ManagerAggregateSnapshot {
        self.manager().await.aggregate_snapshot().await
    }

    pub(crate) fn runtime_snapshot(&self, session_id: Uuid) -> RuntimeSelectionSnapshot {
        let detached = self
            .app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&session_id);
        let has_pty = self
            .app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        RuntimeSelectionSnapshot {
            session_id,
            has_pty,
            detached,
        }
    }

    pub(crate) fn live_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        LiveRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Live)
    }

    pub(crate) fn dormant_decision(&self, session_id: Uuid) -> Option<CommitDecision> {
        DormantRuntimeWitness::from_snapshot(self.runtime_snapshot(session_id))
            .map(CommitDecision::Dormant)
    }

    pub(crate) async fn commit(
        &self,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        self.manager()
            .await
            .commit_selection_transition(&self.capability, decision, cause, mutations)
            .await
    }

    pub(crate) async fn persist(&self, source: SelectionSource, session_id: Option<Uuid>) {
        let manager = self.manager().await;
        if let Err(error) = persist_current_state_result(&manager).await {
            log::warn!(
                "[selection] persistence failed source={} session={:?}: {}",
                source,
                session_id,
                error
            );
        }
    }

    pub(crate) fn publish_selection(&self, payload: &SessionSelection) {
        publish_selection(&self.app, payload);
    }

    pub(crate) fn publish_created(&self, info: &SessionInfo) {
        publish_lifecycle_event(&self.app, "session_created", info);
    }

    pub(crate) fn publish_destroyed(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_destroyed",
            &serde_json::json!({ "id": session_id.to_string() }),
        );
    }

    pub(crate) fn publish_communication_cleared(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_communication_changed",
            &serde_json::json!({
                "sessionId": session_id.to_string(),
                "communication": null,
            }),
        );
    }
}

async fn worker_loop<R: Runtime>(
    app: AppHandle<R>,
    inner: Arc<CoordinatorInner>,
    mut receiver: mpsc::Receiver<CoordinatorEnvelope>,
) {
    let transaction = SelectionTransaction::new(app);
    loop {
        let envelope = tokio::select! {
            biased;
            _ = inner.shutdown.cancelled() => {
                receiver.close();
                drain_after_shutdown(&transaction, &mut receiver).await;
                break;
            }
            envelope = receiver.recv() => envelope,
        };
        let Some(envelope) = envelope else {
            break;
        };
        execute_envelope(&transaction, envelope).await;
    }
}

async fn drain_after_shutdown<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    receiver: &mut mpsc::Receiver<CoordinatorEnvelope>,
) {
    while let Some(envelope) = receiver.recv().await {
        match envelope.job {
            CoordinatorJob::FinalizeCreate { request, response } => {
                execute_rollback_create(transaction, request.binding).await;
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!(
                        "[selection] shutdown finalizer caller dropped session={}",
                        request.binding.session_id()
                    );
                }
            }
            CoordinatorJob::RollbackCreate { binding } => {
                execute_rollback_create(transaction, binding).await;
            }
            CoordinatorJob::Transition { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued transition caller dropped during shutdown");
                }
            }
            CoordinatorJob::Snapshot { response } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued snapshot caller dropped during shutdown");
                }
            }
            CoordinatorJob::RouteLoss { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued route-loss caller dropped during shutdown");
                }
            }
            CoordinatorJob::Destroy { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued destroy caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestartLifecycle { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued restart caller dropped during shutdown");
                }
            }
            CoordinatorJob::RootLifecycle { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued Root caller dropped during shutdown");
                }
            }
            CoordinatorJob::ResourceKill { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued resource-kill caller dropped during shutdown");
                }
            }
            CoordinatorJob::Detach { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued detach caller dropped during shutdown");
                }
            }
            CoordinatorJob::Attach { response, .. } => {
                if response
                    .send(Err(SelectionCoordinatorError::Unavailable.to_string()))
                    .is_err()
                {
                    log::debug!("[selection] queued attach caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestoreBarrier { started, .. } => {
                if started.send(()).is_err() {
                    log::debug!("[selection] restore submitter dropped during shutdown");
                }
            }
        }
    }
}

async fn execute_envelope<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    envelope: CoordinatorEnvelope,
) {
    match envelope.job {
        CoordinatorJob::Transition { request, response } => {
            let result = execute_transition(transaction, request).await;
            if response.send(result).is_err() {
                log::debug!("[selection] transition caller dropped before result delivery");
            }
        }
        CoordinatorJob::Snapshot { response } => {
            let snapshot = transaction.manager().await.selection_payload().await;
            if response.send(Ok(snapshot)).is_err() {
                log::debug!("[selection] snapshot caller dropped before result delivery");
            }
        }
        CoordinatorJob::FinalizeCreate { request, response } => {
            let session_id = request.binding.session_id();
            let result = execute_finalize_create(transaction, request).await;
            if response.send(result).is_err() {
                log::debug!(
                    "[selection] create finalizer caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::RollbackCreate { binding } => {
            execute_rollback_create(transaction, binding).await;
        }
        CoordinatorJob::RouteLoss {
            session_id,
            exit_code,
            response,
        } => {
            let result = execute_route_loss(transaction, session_id, exit_code).await;
            if response.send(result).is_err() {
                log::debug!(
                    "[selection] route-loss caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Destroy { request, response } => {
            let result =
                crate::commands::session::execute_destroy_transaction(transaction, request).await;
            if response.send(result).is_err() {
                log::debug!("[selection] destroy caller dropped before result delivery");
            }
        }
        CoordinatorJob::RestartLifecycle { request, response } => {
            let result =
                crate::commands::session::execute_restart_transaction(transaction, request).await;
            if response.send(result).is_err() {
                log::debug!("[selection] restart caller dropped before result delivery");
            }
        }
        CoordinatorJob::RootLifecycle { request, response } => {
            let result =
                crate::commands::session::execute_root_transaction(transaction, request).await;
            if response.send(result).is_err() {
                log::debug!("[selection] Root caller dropped before result delivery");
            }
        }
        CoordinatorJob::ResourceKill {
            session_id,
            intent,
            response,
        } => {
            let result = crate::commands::resource_monitor::execute_resource_kill_transaction(
                transaction,
                session_id,
                intent,
            )
            .await;
            if response.send(result).is_err() {
                log::debug!(
                    "[selection] resource-kill caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Detach {
            session_id,
            geometry,
            suppress_selection,
            response,
        } => {
            let result = crate::commands::window::execute_detach_transaction(
                transaction,
                session_id,
                geometry,
                suppress_selection,
            )
            .await;
            if response.send(result).is_err() {
                log::debug!(
                    "[selection] detach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Attach {
            session_id,
            response,
        } => {
            let result =
                crate::commands::window::execute_attach_transaction(transaction, session_id).await;
            if response.send(result).is_err() {
                log::debug!(
                    "[selection] attach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::RestoreBarrier { started, release } => {
            if started.send(()).is_err() {
                log::debug!("[selection] restore submitter dropped before barrier start");
            }
            if release.await.is_err() {
                log::warn!("[selection] restore barrier released by dropped owner");
            }
        }
    }
}

async fn execute_transition<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: SelectionRequest,
) -> Result<Option<SessionSelection>, String> {
    match request {
        SelectionRequest::UserSwitch { session_id } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let target = aggregate
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
                .ok_or_else(|| format!("Session not found: {session_id}"))?;
            let runtime = transaction.runtime_snapshot(session_id);
            if runtime.detached {
                let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
                if let Some(window) = transaction.app().get_webview_window(&label) {
                    if let Err(error) = window.set_focus() {
                        log::warn!(
                            "[selection] failed to focus detached target session={} source=userSwitch: {}",
                            session_id,
                            error
                        );
                    }
                }
                return Ok(None);
            }

            let decision = match target.status {
                SessionStatus::Exited(_) => CommitDecision::Dormant(
                    DormantRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session is detached".to_string())?,
                ),
                _ if runtime.has_pty => CommitDecision::Live(
                    LiveRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session has no live PTY".to_string())?,
                ),
                _ => {
                    if aggregate.selection.id() == Some(session_id) {
                        let repair = transaction
                            .commit(
                                CommitDecision::Clear,
                                SelectionCause::LivenessReconcile,
                                LifecycleMutations::default(),
                            )
                            .await?;
                        if let Some(payload) = repair.selection.as_ref() {
                            transaction
                                .persist(SelectionSource::LivenessReconcile, Some(session_id))
                                .await;
                            transaction.publish_selection(payload);
                        }
                    }
                    return Err("Session has no live PTY".to_string());
                }
            };
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::UserSwitch,
                    LifecycleMutations::default(),
                )
                .await?;
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::UserSwitch, Some(session_id))
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
        SelectionRequest::Restore { persisted_target } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let exact = persisted_target.and_then(|target_id| {
                let record = aggregate
                    .sessions
                    .iter()
                    .find(|record| record.id == target_id)?;
                let runtime = transaction.runtime_snapshot(target_id);
                if runtime.detached {
                    return None;
                }
                match record.status {
                    SessionStatus::Exited(_) => transaction.dormant_decision(target_id),
                    _ if runtime.has_pty => transaction.live_decision(target_id),
                    _ => None,
                }
            });
            let decision = exact
                .or_else(|| {
                    aggregate.sessions.iter().find_map(|record| {
                        if matches!(record.status, SessionStatus::Exited(_)) {
                            return None;
                        }
                        transaction.live_decision(record.id)
                    })
                })
                .unwrap_or(CommitDecision::Clear);
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::Restore,
                    LifecycleMutations::default(),
                )
                .await?;
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::Restore, payload.id())
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
    }
}

async fn execute_finalize_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: FinalizeCreateRequest,
) -> Result<SessionInfo, String> {
    let session_id = request.binding.session_id();
    if transaction
        .manager()
        .await
        .get_pending_session(request.binding)
        .await
        .is_none()
    {
        execute_rollback_create(transaction, request.binding).await;
        return Err("created session pending record is unavailable".to_string());
    }
    let runtime = transaction.runtime_snapshot(session_id);
    let live = LiveRuntimeWitness::from_snapshot(runtime)
        .ok_or_else(|| "created session is not displayable".to_string());
    let live = match live {
        Ok(live) => live,
        Err(error) => {
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let current = transaction.manager().await.selection_payload().await;
    let should_select =
        request
            .auto_select_precondition
            .as_ref()
            .is_some_and(|(epoch, revision)| {
                current.mode() == SelectionMode::None
                    && current.epoch() == epoch
                    && current.revision() == *revision
            });
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(request.binding, live);
    let decision = if should_select {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction.commit(decision, request.cause, mutations).await {
        Ok(committed) => committed,
        Err(error) => {
            log::warn!(
                "[selection] create finalization commit rejected session={}: {}",
                session_id,
                error
            );
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == session_id.to_string())
        .cloned()
        .ok_or_else(|| "create finalization did not publish a row".to_string())?;
    transaction
        .persist(SelectionSource::SessionCreated, Some(session_id))
        .await;
    transaction.publish_created(&info);
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    for warning in request.warnings {
        emit_session_warning(transaction.app(), warning);
    }
    Ok(info)
}

async fn execute_restore_dormant<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DormantRestoreRequest,
) -> Result<SessionInfo, String> {
    let id = Uuid::new_v4();
    let binding = PendingCreateBinding::new(id, Uuid::new_v4());
    let persisted = request.persisted;
    let exit_code = match persisted.status {
        Some(SessionStatus::Exited(code)) => code,
        _ => 0,
    };
    let session = crate::session::session::Session {
        id,
        name: persisted.name,
        shell: persisted.shell,
        shell_args: persisted.shell_args,
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: chrono::Utc::now(),
        working_directory: request.working_directory,
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: persisted.communication,
        pending_review: false,
        last_prompt: persisted.last_prompt,
        agent_id: persisted.agent_id,
        agent_label: persisted.agent_label,
        git_repos: persisted.git_repos,
        is_coordinator: request.is_coordinator,
        is_root_agent: request.is_root_agent,
        git_repos_gen: 0,
        token: Uuid::new_v4(),
        agent_kind: None,
        requested_profile: persisted.requested_profile,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        telegram_bot_id: persisted.telegram_bot_id,
        was_detached: persisted.was_detached,
        detached_geometry: persisted.detached_geometry,
        start_fresh_on_restore: persisted.start_fresh_on_restore,
    };
    transaction
        .manager()
        .await
        .insert_transaction_pending_record(session, binding)
        .await?;
    let witness = DormantRuntimeWitness::from_snapshot(transaction.runtime_snapshot(id))
        .ok_or_else(|| "restored dormant record is runtime-detached".to_string())?;
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_dormant(binding, witness, exit_code);
    let committed = transaction
        .commit(CommitDecision::Keep, SelectionCause::Restore, mutations)
        .await?;
    let info = committed
        .finalized_rows
        .into_iter()
        .find(|row| row.id == id.to_string())
        .ok_or_else(|| "dormant restore finalization did not publish a row".to_string())?;
    transaction
        .persist(SelectionSource::Restore, Some(id))
        .await;
    transaction.publish_created(&info);
    Ok(info)
}

async fn execute_rollback_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    binding: PendingCreateBinding,
) {
    let manager = transaction.manager().await;
    let pty = transaction
        .app()
        .try_state::<Arc<Mutex<PtyManager>>>()
        .map(|state| state.inner().clone());
    execute_rollback_create_parts(manager, pty, binding).await;
}

async fn execute_rollback_create_parts(
    manager: SessionManager,
    pty: Option<Arc<Mutex<PtyManager>>>,
    binding: PendingCreateBinding,
) {
    let session_id = binding.session_id();
    let backend_kind = manager
        .get_pending_session(binding)
        .await
        .map(|session| session.backend_kind);
    if let Some(pty) = pty {
        let pty = pty.lock().unwrap_or_else(|error| error.into_inner());
        let kill_result = match backend_kind {
            Some(kind) => Some(pty.kill_for_kind(session_id, kind)),
            None if pty.has_session(session_id) => Some(pty.kill(session_id)),
            None => None,
        };
        if let Some(Err(error)) = kill_result {
            log::debug!(
                "[selection] pending create PTY cleanup session={} result={}",
                session_id,
                error
            );
        }
    }
    if backend_kind.is_none() {
        return;
    }
    if let Err(error) = manager.rollback_pending_create(binding).await {
        log::warn!(
            "[selection] pending create manager rollback failed session={}: {}",
            session_id,
            error
        );
    }
}

fn begin_container_shutdown(inner: &Arc<CoordinatorInner>, deadline: Instant) -> bool {
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable during shutdown signal: {}",
                error
            );
            return false;
        }
    };
    if let Some(container_backend) = container_backend {
        return container_backend.begin_shutdown(deadline);
    }
    true
}

async fn cleanup_pending_creates_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let pty = match inner.cleanup_pty.try_lock() {
        Ok(pty) => pty.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] pending-create PTY ownership lock unavailable: {}",
                error
            );
            return vec!["reason=pending-pty-owner state=retained".to_string()];
        }
    };
    let Some(pty) = pty else {
        log::error!(
            "[selection] pending-create PTY owner unavailable after worker join state=retained"
        );
        return vec!["reason=pending-pty-owner state=retained".to_string()];
    };
    let manager = Arc::clone(&inner.manager);
    let mut cleanup = tokio::spawn(async move {
        let manager = manager.read().await.clone();
        let bindings = manager.pending_create_bindings().await;
        if !bindings.is_empty() {
            log::warn!(
                "[selection] cleaning {} pending create(s) after worker join",
                bindings.len()
            );
        }
        let mut retained = Vec::new();
        for binding in bindings {
            let session_id = binding.session_id();
            let Some(backend_kind) = manager
                .get_pending_session(binding)
                .await
                .map(|session| session.backend_kind)
            else {
                continue;
            };
            let kill_pty = Arc::clone(&pty);
            let kill = tokio::task::spawn_blocking(move || {
                kill_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .kill_for_kind(session_id, backend_kind)
            })
            .await;
            match kill {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup failed session={} reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
                Err(error) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup task failed session={} reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
            }
            if let Err(error) = manager.rollback_pending_create(binding).await {
                log::warn!(
                    "[selection] pending create manager rollback failed session={} reason=pending-manager-rollback state=retained error={}",
                    session_id,
                    error
                );
                retained.push(format!(
                    "session={session_id} reason=pending-manager-rollback state=retained"
                ));
            }
        }
        retained
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut cleanup).await {
        Ok(Ok(retained)) => retained,
        Ok(Err(error)) => {
            log::error!(
                "[selection] pending-create cleanup owner failed reason=pending-cleanup-task state=retained error={}",
                error
            );
            vec!["reason=pending-cleanup-task state=retained".to_string()]
        }
        Err(_) => {
            log::error!(
                "[selection] pending-create cleanup owner retained after absolute deadline reason=pending-cleanup-await state=retained"
            );
            inner
                .retained_rollback_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(cleanup);
            vec!["reason=pending-cleanup-await state=retained".to_string()]
        }
    }
}

async fn seal_and_drain_container_shutdown_work_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let mut retained = Vec::new();
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable before bounded drain: {}",
                error
            );
            retained.push("reason=container-drain-owner state=retained".to_string());
            return retained;
        }
    };
    let Some(container_backend) = container_backend else {
        return retained;
    };
    let mut task = tokio::task::spawn_blocking(move || {
        container_backend.seal_and_drain_shutdown_work_blocking(deadline)
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut task).await {
        Ok(Ok(report)) => retained.extend(report.retained),
        Ok(Err(error)) => {
            log::error!(
                "[selection] container shutdown-work drain failed after coordinator join: {}",
                error
            );
            retained.push("reason=container-drain-task state=retained".to_string());
        }
        Err(_) => {
            log::error!(
                "[selection] container shutdown-work blocking task retained after absolute deadline reason=container-drain-await state=retained"
            );
            retained.push("reason=container-drain-await state=retained".to_string());
            inner
                .retained_cleanup_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(task);
        }
    }
    retained
}

async fn execute_route_loss<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    exit_code: i32,
) -> Result<(), String> {
    let aggregate = transaction.aggregate_snapshot().await;
    if aggregate.pending_ids.contains(&session_id) {
        return Ok(());
    }
    let Some(record) = aggregate
        .sessions
        .iter()
        .find(|record| record.id == session_id)
    else {
        return Ok(());
    };
    if matches!(record.status, SessionStatus::Exited(_)) {
        return Ok(());
    }
    let mut runtime = transaction.runtime_snapshot(session_id);
    if runtime.detached {
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[selection] route-loss detached window close failed session={}: {}",
                    session_id,
                    error
                );
            }
        }
        runtime = transaction.runtime_snapshot(session_id);
    }
    let decision = if aggregate.selection.id() == Some(session_id) {
        CommitDecision::Dormant(
            DormantRuntimeWitness::from_snapshot(runtime)
                .ok_or_else(|| "route-loss target remained detached".to_string())?,
        )
    } else {
        CommitDecision::Keep
    };
    let mut mutations = LifecycleMutations::default();
    mutations.mark_exited(session_id, exit_code);
    let committed = transaction
        .commit(decision, SelectionCause::LivenessReconcile, mutations)
        .await?;
    if committed.changed_rows.is_empty() {
        return Ok(());
    }
    transaction
        .persist(SelectionSource::LivenessReconcile, Some(session_id))
        .await;
    transaction.publish_destroyed(session_id);
    for row in &committed.changed_rows {
        transaction.publish_created(row);
    }
    for cleared in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*cleared);
    }
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    Ok(())
}

fn remove_critical_key(inner: &Arc<CoordinatorInner>, key: CriticalAdmissionKey) {
    match inner.critical_keys.lock() {
        Ok(mut keys) => {
            keys.remove(&key);
        }
        Err(error) => {
            log::error!(
                "[selection] critical-key lock poisoned session={} kind={:?}: {}",
                key.session_id,
                key.kind,
                error
            );
        }
    }
}

fn publish_selection<R: Runtime>(app: &AppHandle<R>, payload: &SessionSelection) {
    publish_lifecycle_event(app, "session_switched", payload);
}

fn publish_lifecycle_event<R: Runtime, T: Serialize>(
    app: &AppHandle<R>,
    event: &'static str,
    payload: &T,
) {
    let value = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(error) => {
            log::error!(
                "[selection] failed to serialize event={} payload: {}",
                event,
                error
            );
            return;
        }
    };
    if let Err(error) = app.emit(event, value.clone()) {
        log::error!(
            "[selection] Tauri publication failed event={}: {}",
            event,
            error
        );
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event(event, &value);
    } else {
        log::debug!(
            "[selection] WebSocket broadcaster unavailable event={}",
            event
        );
    }
}

pub(crate) fn publish_session_communication<R: Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    communication: Option<&crate::session::session::SessionCommunication>,
) {
    publish_lifecycle_event(
        app,
        "session_communication_changed",
        &serde_json::json!({
            "sessionId": session_id.to_string(),
            "communication": communication,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    #[derive(Default)]
    struct LifecycleTestBackend {
        live: Mutex<HashSet<Uuid>>,
        kills: Mutex<std::collections::HashMap<Uuid, usize>>,
    }

    impl LifecycleTestBackend {
        fn set_live(&self, session_id: Uuid, live: bool) {
            let mut sessions = self.live.lock().unwrap();
            if live {
                sessions.insert(session_id);
            } else {
                sessions.remove(&session_id);
            }
        }

        fn kill_count(&self, session_id: Uuid) -> usize {
            self.kills
                .lock()
                .unwrap()
                .get(&session_id)
                .copied()
                .unwrap_or_default()
        }
    }

    struct GatedStopRuntime {
        stop_started: Mutex<Option<oneshot::Sender<()>>>,
        stop_calls: AtomicUsize,
        stop_hold: Duration,
        active_stops: AtomicUsize,
        deadline_seen: AtomicBool,
    }

    struct GatedStartStopRuntime {
        start_started: Mutex<Option<oneshot::Sender<()>>>,
        stop_started: Mutex<Option<oneshot::Sender<()>>>,
        start_calls: AtomicUsize,
        stop_calls: AtomicUsize,
        stop_hold: Duration,
        active_starts: AtomicUsize,
        active_stops: AtomicUsize,
        deadline_seen: AtomicBool,
        stop_outcomes: Mutex<std::collections::VecDeque<CanceledStartStopOutcome>>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CanceledStartStopOutcome {
        Success,
        Error,
        Panic,
    }

    struct AtomicActivityGuard<'a> {
        active: &'a AtomicUsize,
    }

    impl<'a> AtomicActivityGuard<'a> {
        fn enter(active: &'a AtomicUsize) -> Self {
            active.fetch_add(1, Ordering::SeqCst);
            Self { active }
        }
    }

    impl Drop for AtomicActivityGuard<'_> {
        fn drop(&mut self) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn hold_runtime_stop_until_budget(
        control: &crate::pty::container_runtime::ContainerRuntimeControl,
        requested_hold: Duration,
    ) {
        if requested_hold.is_zero() {
            return;
        }
        let _ = control.wait_for_shutdown();
        let remaining = control.remaining().unwrap_or(requested_hold);
        let hold = requested_hold.min(remaining.saturating_sub(Duration::from_millis(20)));
        if !hold.is_zero() {
            std::thread::sleep(hold);
        }
    }

    impl crate::pty::container_runtime::ContainerRuntime for GatedStopRuntime {
        fn start(
            &self,
            request: crate::pty::container_runtime::ContainerStartRequest,
            _control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
                session_id: request.session_id,
                container_id: format!("container-{}", request.session_id),
            })
        }

        fn stop(
            &self,
            _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
            _timeout: Duration,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<(), crate::errors::AppError> {
            let _active = AtomicActivityGuard::enter(&self.active_stops);
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self
                .stop_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            hold_runtime_stop_until_budget(control, self.stop_hold);
            self.deadline_seen
                .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
            Ok(())
        }

        fn cleanup_labeled_orphans(
            &self,
            _live_sessions: &HashSet<Uuid>,
            _timeout: Duration,
        ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
        }
    }

    impl crate::pty::container_runtime::ContainerRuntime for GatedStartStopRuntime {
        fn start(
            &self,
            request: crate::pty::container_runtime::ContainerStartRequest,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
        {
            let _active = AtomicActivityGuard::enter(&self.active_starts);
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self
                .start_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            let _ = control.wait_for_shutdown();
            Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
                session_id: request.session_id,
                container_id: format!("container-{}", request.session_id),
            })
        }

        fn stop(
            &self,
            _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
            _timeout: Duration,
            control: &crate::pty::container_runtime::ContainerRuntimeControl,
        ) -> Result<(), crate::errors::AppError> {
            let _active = AtomicActivityGuard::enter(&self.active_stops);
            let call = self.stop_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(started) = self
                .stop_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = started.send(());
            }
            if call == 1 {
                hold_runtime_stop_until_budget(control, self.stop_hold);
            }
            self.deadline_seen
                .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
            match self
                .stop_outcomes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
                .unwrap_or(CanceledStartStopOutcome::Success)
            {
                CanceledStartStopOutcome::Success => Ok(()),
                CanceledStartStopOutcome::Error => Err(crate::errors::AppError::Other(
                    "injected canceled-start stop failure".to_string(),
                )),
                CanceledStartStopOutcome::Panic => {
                    panic!("injected canceled-start stop panic")
                }
            }
        }

        fn cleanup_labeled_orphans(
            &self,
            _live_sessions: &HashSet<Uuid>,
            _timeout: Duration,
        ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
        {
            Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
        }
    }

    fn real_container_spawn_spec(session_id: Uuid) -> crate::pty::backend::BackendSpawnSpec {
        crate::pty::backend::BackendSpawnSpec {
            id: session_id,
            agent_id: None,
            coding_agent: None,
            cmd: "container".to_string(),
            args: Vec::new(),
            cwd: "C:/repo/.ac/wg-1/__agent_dev".to_string(),
            selected_cwd: None,
            cols: 120,
            rows: 30,
            container_image: Some("agentscommander/test:latest".to_string()),
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            env_unset: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target: crate::pty::output::PtyOutputTarget::noop(),
            resource_registration: None,
            logical_resource_slot: None,
            container_credential: None,
            container_repo_mounts: Vec::new(),
        }
    }

    impl crate::pty::backend::PtyBackend for LifecycleTestBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.set_live(spec.id, true);
                Ok(())
            })
        }

        fn write(&self, id: Uuid, _data: &[u8]) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            self.write(id, &[])
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            *self.kills.lock().unwrap().entry(id).or_default() += 1;
            self.set_live(id, false);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.has_session(id).then_some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[derive(Clone, Copy)]
    enum CriticalWaitPoint {
        AdmissionPermit,
        QueueSlot,
    }

    async fn assert_cancelled_critical_waiter_releases_key(
        kind: CriticalAdmissionKind,
        wait_point: CriticalWaitPoint,
    ) {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-cancellation".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create critical-cancellation fixture");
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-cancellation app");
        coordinator
            .start(app.handle().clone())
            .expect("start critical-cancellation coordinator");
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| {
                coordinator
                    .reserve_auto_close()
                    .expect("reserve physical queue slot")
            })
            .collect::<Vec<_>>();
        let held_admission = match wait_point {
            CriticalWaitPoint::AdmissionPermit => Some(
                Arc::clone(&coordinator.inner.admission)
                    .try_acquire_owned()
                    .expect("hold final logical admission permit"),
            ),
            CriticalWaitPoint::QueueSlot => None,
        };

        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.critical_probe_for_test(session.id, kind).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let registered = coordinator.critical_key_registered_for_test(session.id, kind);
                let reached_wait = match wait_point {
                    CriticalWaitPoint::AdmissionPermit => registered,
                    CriticalWaitPoint::QueueSlot => {
                        registered && coordinator.inner.admission.available_permits() == 0
                    }
                };
                if reached_wait {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("critical waiter reaches requested cancellation point");

        waiter.abort();
        assert!(waiter
            .await
            .expect_err("waiter must be aborted")
            .is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.critical_key_registered_for_test(session.id, kind) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted waiter removes its critical admission key");

        drop(held_admission);
        drop(reservations);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                coordinator.critical_probe_for_test(session.id, kind),
            )
            .await
            .expect("fresh same-kind submission completes")
            .expect("fresh same-kind submission succeeds"),
            CriticalAdmissionOutcome::Completed(())
        );
        assert!(!coordinator.critical_key_registered_for_test(session.id, kind));
        coordinator.close_and_join().await;
    }

    async fn assert_pending_container_shutdown_waits_for_stop(capped: bool) {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_runtime::ContainerRuntimeHandle;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (stop_started, stop_started_rx) = oneshot::channel();
        let close_budget = Duration::from_millis(200);
        let runtime = Arc::new(GatedStopRuntime {
            stop_started: Mutex::new(Some(stop_started)),
            stop_calls: AtomicUsize::new(0),
            stop_hold: if capped {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(10)
            },
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
        });
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build pending-container shutdown app");
        coordinator
            .start(app.handle().clone())
            .expect("start pending-container shutdown coordinator");
        let mut restore_guard = Some(
            coordinator
                .submit_restore_first()
                .await
                .expect("hold restore barrier"),
        );
        let manager_handle = manager.read().await.clone();
        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .expect("reserve pending-container finalizer");
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "container".to_string(),
                Vec::new(),
                "C:/pending-container-shutdown".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await
            .expect("create pending-container manager row");
        let _transport_receiver =
            container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id: pending.id,
                container_id: format!("container-{}", pending.id),
            });
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_route(pending.id, SessionBackendKind::ContainerTransport);

        let finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
        let close_started = Instant::now();
        let close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                if capped {
                    coordinator.close_and_join_with_budget(close_budget).await;
                } else {
                    coordinator.close_and_join().await;
                }
            })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator.inner.shutdown.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending-container shutdown signal becomes visible");
        if !capped {
            restore_guard
                .take()
                .expect("normal-drain restore guard")
                .finish();
        }

        tokio::time::timeout(Duration::from_secs(2), close)
            .await
            .expect("coordinator close obeys the shared shutdown deadline")
            .expect("join coordinator close task");
        let close_elapsed = close_started.elapsed();
        let close_bound = if capped {
            close_budget + Duration::from_millis(550)
        } else {
            Duration::from_secs(1)
        };
        assert!(
            close_elapsed <= close_bound,
            "coordinator close elapsed {close_elapsed:?}, bound {close_bound:?}"
        );
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }
        assert_eq!(
            finalizer
                .await
                .expect("join pending-container finalizer")
                .expect_err("shutdown finalizer returns unavailable"),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(!container_backend.contains_transport_state_for_test(pending.id));
        assert_eq!(container_backend.detached_cleanup_count_for_test(), 0);
        assert_eq!(
            container_backend.retained_runtime_cleanup_sessions_for_test(),
            vec![pending.id],
            "post-seal pending cleanup must remain owned for the authorized global sweep: {:?}",
            container_backend.retained_cleanup_contexts_for_test()
        );
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(!pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(pending.id));

        let global_report = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(
            global_report.terminal,
            "retained={:?}",
            global_report.retained
        );
        tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
            .await
            .expect("global sweep invokes the retained container stop")
            .expect("container stop-start signal is delivered");
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            1,
            "the single production global sweep must stop the retained handle exactly once"
        );
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(runtime.deadline_seen.load(Ordering::SeqCst));
        assert!(container_backend
            .retained_cleanup_sessions_for_test()
            .is_empty());
    }

    async fn assert_real_pending_container_start_shutdown_waits_for_stop(
        capped: bool,
        fail_worker_spawn: bool,
        first_stop_outcome: CanceledStartStopOutcome,
    ) {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_tokens::ContainerApiTokenManager;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (start_started, start_started_rx) = oneshot::channel();
        let (stop_started, stop_started_rx) = oneshot::channel();
        let close_budget = Duration::from_millis(200);
        let runtime = Arc::new(GatedStartStopRuntime {
            start_started: Mutex::new(Some(start_started)),
            stop_started: Mutex::new(Some(stop_started)),
            start_calls: AtomicUsize::new(0),
            stop_calls: AtomicUsize::new(0),
            stop_hold: if capped {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(10)
            },
            active_starts: AtomicUsize::new(0),
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
            stop_outcomes: Mutex::new(std::collections::VecDeque::from([
                first_stop_outcome,
                CanceledStartStopOutcome::Success,
            ])),
        });
        let token_dir = tempfile::TempDir::new().expect("create container token directory");
        let token_manager =
            ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let runtime_settings = crate::config::settings::AppSettings {
            api_server_enabled: true,
            api_server_bind: "0.0.0.0".to_string(),
            api_server_port: 8765,
            ..crate::config::settings::AppSettings::default()
        };
        let mut container_backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            Some(token_manager),
        );
        container_backend.set_runtime_settings_for_test(runtime_settings);
        if fail_worker_spawn {
            container_backend.inject_shutdown_worker_spawn_failure_for_test();
        }
        let container_backend = Arc::new(container_backend);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build real pending-container shutdown app");
        coordinator
            .start(app.handle().clone())
            .expect("start real pending-container shutdown coordinator");
        let mut restore_guard = Some(
            coordinator
                .submit_restore_first()
                .await
                .expect("hold restore barrier"),
        );
        let manager_handle = manager.read().await.clone();
        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .expect("reserve real pending-container finalizer");
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "container".to_string(),
                Vec::new(),
                "C:/repo/.ac/wg-1/__agent_dev".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await
            .expect("create real pending-container manager row");
        let pending_id = pending.id;
        let create_shutdown = coordinator.inner.shutdown.clone();
        let create_pty = Arc::clone(&pty);
        let mut create = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build real-container create test runtime");
            runtime.block_on(async move {
                let _ticket = ticket;
                let _spawn_mark = create_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .mark_spawning("C:/repo/.ac/wg-1/__agent_dev", "container");
                tokio::select! {
                    biased;
                    _ = create_shutdown.cancelled() => {
                        Err(SelectionCoordinatorError::Unavailable.to_string())
                    }
                    result = PtyManager::spawn(
                        &create_pty,
                        SessionBackendKind::ContainerTransport,
                        real_container_spawn_spec(pending_id),
                    ) => result.map_err(|error| error.to_string()),
                }
            })
        });

        tokio::select! {
            result = &mut create => {
                panic!("real container create ended before runtime start: {result:?}");
            }
            started = tokio::time::timeout(Duration::from_secs(5), start_started_rx) => {
                started
                    .expect("runtime start reaches blocking section")
                    .expect("runtime start witness is delivered");
            }
        }
        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert!(container_backend.contains_transport_state_for_test(pending_id));
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.contains(&pending_id));
        assert!(aggregate.sessions.is_empty());
        let (pending_spawns, live_routes) = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archive_liveness(&[pending_id]);
        assert_eq!(pending_spawns.len(), 1);
        assert_eq!(live_routes, vec![false]);
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (false, 1, 1),
            "the producer and blocking start must be shutdown-owned before runtime.start"
        );

        let close_started = Instant::now();
        let close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                if capped {
                    coordinator.close_and_join_with_budget(close_budget).await;
                } else {
                    coordinator.close_and_join().await;
                }
            })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator.inner.shutdown.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real pending-container shutdown signal becomes visible");
        let create_error = (&mut create)
            .await
            .expect("join canceled real container create")
            .expect_err("shutdown cancels the real container create");
        if fail_worker_spawn {
            assert!(
                create_error == SelectionCoordinatorError::Unavailable.to_string()
                    || create_error == "container runtime start was canceled",
                "unexpected synchronous-fallback cancellation error: {create_error}"
            );
        } else {
            assert_eq!(
                create_error,
                SelectionCoordinatorError::Unavailable.to_string()
            );
        }
        if !capped {
            restore_guard
                .take()
                .expect("normal-drain restore guard")
                .finish();
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager_handle
                    .aggregate_snapshot()
                    .await
                    .pending_ids
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending manager row is rolled back during shutdown");
        tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
            .await
            .expect("late runtime handle stop starts")
            .expect("late runtime handle stop witness is delivered");
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(2), close)
            .await
            .expect("real pending-container close obeys the shared deadline")
            .expect("join real pending-container close task");
        let close_elapsed = close_started.elapsed();
        let close_bound = if capped {
            close_budget + Duration::from_millis(550)
        } else {
            Duration::from_secs(1)
        };
        assert!(
            close_elapsed <= close_bound,
            "real container close elapsed {close_elapsed:?}, bound {close_bound:?}"
        );
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }

        assert_eq!(runtime.start_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_starts.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(runtime.deadline_seen.load(Ordering::SeqCst));
        assert!(!container_backend.contains_transport_state_for_test(pending_id));
        let retry_expected = first_stop_outcome != CanceledStartStopOutcome::Success;
        let retained_contexts = container_backend.retained_cleanup_contexts_for_test();
        assert_eq!(
            container_backend.retained_runtime_cleanup_sessions_for_test(),
            if retry_expected {
                vec![pending_id]
            } else {
                Vec::new()
            },
            "the deterministic canceled-start handle must remain owned after a nonterminal stop: {:?}",
            retained_contexts
        );
        assert!(
            retained_contexts.iter().all(|context| {
                context.contains("runtimeHandle=false")
                    || (retry_expected
                        && context.contains(&pending_id.to_string())
                        && context.contains("runtimeHandle=true"))
            }),
            "retained cleanup must identify runtime-backed ownership separately from non-runtime residue: {retained_contexts:?}"
        );
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (true, 0, 0)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        let (pending_spawns, live_routes) = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archive_liveness(&[pending_id]);
        assert!(pending_spawns.is_empty());
        assert_eq!(live_routes, vec![false]);
        assert!(pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .backend_kind(pending_id)
            .is_none());

        if fail_worker_spawn {
            container_backend.clear_shutdown_worker_spawn_failure_for_test();
        }
        let global_report = pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(
            global_report.terminal,
            "retained={:?}",
            global_report.retained
        );
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            if retry_expected { 2 } else { 1 },
            "the single production global sweep must retry only a nonterminal deterministic owner"
        );
        assert!(container_backend
            .retained_cleanup_sessions_for_test()
            .is_empty());
    }

    fn production_prefix(source: &str) -> &str {
        [
            "\n#[cfg(test)]\nmod tests",
            "\n#[cfg(test)]\nimpl SessionManager",
        ]
        .into_iter()
        .filter_map(|marker| source.find(marker))
        .min()
        .map_or(source, |index| &source[..index])
    }

    fn function_name(line: &str) -> Option<String> {
        let start = line.find("fn ")? + 3;
        let name = line[start..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect::<String>();
        (!name.is_empty()).then_some(name)
    }

    fn ownership_violations(files: &[(String, String)]) -> Vec<String> {
        let mut violations = Vec::new();
        for (path, source) in files {
            let mut function = String::new();
            let normalized = source.replace("\r\n", "\n");
            for (index, line) in production_prefix(&normalized).lines().enumerate() {
                if let Some(name) = function_name(line) {
                    function = name;
                }
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if line.contains("= SessionStatus::Active")
                    && !(path.ends_with("session/manager.rs")
                        && function == "commit_selection_transition")
                {
                    violations.push(format!(
                        "{path}:{} Active assignment in {function}",
                        index + 1
                    ));
                }

                for event in [
                    "session_switched",
                    "session_created",
                    "session_destroyed",
                    "session_communication_changed",
                ] {
                    if !line.contains(&format!("\"{event}\"")) {
                        continue;
                    }
                    let allowed = path.ends_with("session/selection.rs")
                        && matches!(
                            function.as_str(),
                            "publish_selection"
                                | "publish_created"
                                | "publish_destroyed"
                                | "publish_communication_cleared"
                                | "publish_session_communication"
                        );
                    if !allowed {
                        violations
                            .push(format!("{path}:{} {event} owner is {function}", index + 1));
                    }
                }

                if line.contains("pub async fn mark_exited(&self")
                    || line.contains("pub async fn get_active(&self")
                    || line.contains("pub async fn switch_session(&self")
                    || line.contains("pub async fn set_active_only(&self")
                    || line.contains("pub async fn clear_active(&self")
                    || line.contains("pub async fn clear_active_if(&self")
                {
                    violations.push(format!(
                        "{path}:{} removed production manager mutator {function}",
                        index + 1
                    ));
                }
            }
        }
        violations
    }

    fn collect_rust_sources(dir: &Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
            let entry = entry.expect("read Rust source entry");
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push((
                    path.to_string_lossy().replace('\\', "/"),
                    std::fs::read_to_string(&path).expect("read Rust source file"),
                ));
            }
        }
    }

    fn coordinator_job_declaration(source: &str) -> &str {
        let start = source
            .find("enum CoordinatorJob")
            .expect("CoordinatorJob declaration");
        let declaration = &source[start..];
        let open = declaration.find('{').expect("CoordinatorJob opening brace");
        let mut depth = 0usize;
        for (offset, character) in declaration[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &declaration[..open + offset + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("CoordinatorJob closing brace");
    }

    fn coordinator_job_handle_violations(source: &str) -> Vec<&'static str> {
        let declaration = coordinator_job_declaration(source);
        [
            "SessionManager",
            "PtyManager",
            "AppHandle",
            "SelectionTransaction",
            "BoxFuture",
            "dyn Future",
            "FnOnce",
        ]
        .into_iter()
        .filter(|forbidden| declaration.contains(forbidden))
        .collect()
    }

    #[test]
    fn production_selection_and_lifecycle_sources_have_one_owner() {
        let mut files = Vec::new();
        collect_rust_sources(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let violations = ownership_violations(&files);
        assert!(
            violations.is_empty(),
            "production lifecycle ownership violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn source_ownership_sentinel_rejects_each_one_line_mutation() {
        let mutations = [
            "fn rogue() { row.status = SessionStatus::Active; }",
            "fn rogue() { app.emit(\"session_switched\", payload); }",
            "fn rogue() { app.emit(\"session_created\", payload); }",
            "fn rogue() { app.emit(\"session_destroyed\", payload); }",
            "fn rogue() { app.emit(\"session_communication_changed\", payload); }",
            "pub async fn mark_exited(&self) {}",
            "pub async fn get_active(&self) {}",
            "pub async fn switch_session(&self) {}",
            "pub async fn set_active_only(&self) {}",
            "pub async fn clear_active(&self) {}",
            "pub async fn clear_active_if(&self) {}",
        ];
        for mutation in mutations {
            let files = vec![("src/commands/session.rs".to_string(), mutation.to_string())];
            assert!(
                !ownership_violations(&files).is_empty(),
                "sentinel accepted mutation: {mutation}"
            );
        }
        let web_forgery = vec![(
            "src/web/commands.rs".to_string(),
            "fn validate_client_broadcast() { allow(\"session_switched\"); }".to_string(),
        )];
        assert!(
            !ownership_violations(&web_forgery).is_empty(),
            "sentinel accepted a lifecycle name in the client allowlist"
        );
    }

    #[test]
    fn coordinator_jobs_are_typed_data_without_managed_handles_or_futures() {
        let source = include_str!("selection.rs");
        assert!(
            coordinator_job_handle_violations(source).is_empty(),
            "CoordinatorJob contains a managed handle or arbitrary executable field: {:?}",
            coordinator_job_handle_violations(source)
        );
        for forbidden in [
            "SessionManager",
            "PtyManager",
            "AppHandle",
            "SelectionTransaction",
            "BoxFuture",
            "dyn Future",
            "FnOnce",
        ] {
            let mutated = format!("enum CoordinatorJob {{ Rogue {{ value: {forbidden} }} }}");
            assert_eq!(
                coordinator_job_handle_violations(&mutated),
                vec![forbidden],
                "sentinel accepted forbidden CoordinatorJob field {forbidden}"
            );
        }
    }

    #[test]
    fn initial_payload_matches_contract() {
        let epoch = Uuid::new_v4();
        let payload = SessionSelection::initial(epoch);
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["epoch"], epoch.to_string());
        assert_eq!(value["source"], "initialHydration");
        assert_eq!(value["userInitiated"], false);
        assert_eq!(value["revision"], 0);
        assert_eq!(value["mode"], "none");
        assert!(value["id"].is_null());
        assert!(value["status"].is_null());
        assert_eq!(value["hasPty"], false);
        assert_eq!(value["detached"], false);
        assert_eq!(value["displayable"], false);
    }

    #[test]
    fn payload_mode_invariants_and_epoch_are_stable() {
        let epoch = Uuid::new_v4();
        let id = Uuid::new_v4();
        let none = SessionSelection::none(epoch, 1, SelectionCause::AutoClose);
        let live = SessionSelection::live(epoch, 2, SelectionCause::UserSwitch, id);
        let dormant =
            SessionSelection::dormant(epoch, 3, SelectionCause::LivenessReconcile, id, 19, false);

        assert_eq!(none.epoch(), live.epoch());
        assert_eq!(live.epoch(), dormant.epoch());
        assert_eq!(none.mode(), SelectionMode::None);
        assert_eq!(none.id(), None);
        assert_eq!(none.status(), None);
        assert!(!none.has_pty() && !none.detached() && !none.displayable());
        assert_eq!(live.mode(), SelectionMode::Live);
        assert_eq!(live.id(), Some(id));
        assert_eq!(live.status(), Some(&SessionStatus::Active));
        assert!(live.has_pty() && !live.detached() && live.displayable());
        assert_eq!(dormant.mode(), SelectionMode::Dormant);
        assert_eq!(dormant.id(), Some(id));
        assert_eq!(dormant.status(), Some(&SessionStatus::Exited(19)));
        assert!(!dormant.has_pty() && !dormant.detached() && !dormant.displayable());

        let serialized = serde_json::to_value(dormant).expect("serialize dormant payload");
        assert_eq!(serialized["source"], "livenessReconcile");
        assert_eq!(serialized["userInitiated"], false);
        assert_eq!(serialized["mode"], "dormant");
    }

    #[test]
    fn authoritative_publisher_sends_byte_equivalent_tauri_and_websocket_payloads() {
        use crate::web::broadcast::WsOutMsg;
        use tauri::Listener;

        let broadcaster = WsBroadcaster::new();
        let mut web = broadcaster.subscribe();
        let app = tauri::test::mock_builder()
            .manage(broadcaster)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build publisher test app");
        let (native_sender, native_receiver) = std::sync::mpsc::channel();
        app.listen_any("session_switched", move |event| {
            let _ = native_sender.send(event.payload().to_string());
        });

        let payload = SessionSelection::live(
            Uuid::new_v4(),
            41,
            SelectionCause::UserSwitch,
            Uuid::new_v4(),
        );
        publish_selection(app.handle(), &payload);
        let native: serde_json::Value = serde_json::from_str(
            &native_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("native selection event"),
        )
        .expect("parse native selection");
        let websocket = match web.try_recv().expect("websocket selection event") {
            WsOutMsg::Text(text) => {
                serde_json::from_str::<serde_json::Value>(&text).expect("parse websocket selection")
            }
            other => panic!("expected websocket text event, got {other:?}"),
        };
        assert_eq!(websocket["event"], "session_switched");
        assert_eq!(websocket["payload"], native);
        assert_eq!(native, serde_json::to_value(payload).unwrap());
    }

    #[test]
    fn sealed_causes_pin_source_and_user_intent() {
        let cases = [
            (
                SelectionCause::UserSwitch,
                SelectionSource::UserSwitch,
                true,
            ),
            (
                SelectionCause::ManualClose,
                SelectionSource::ManualClose,
                true,
            ),
            (SelectionCause::AutoClose, SelectionSource::AutoClose, false),
            (SelectionCause::Restore, SelectionSource::Restore, false),
            (SelectionCause::Detach, SelectionSource::Detach, true),
            (SelectionCause::Attach, SelectionSource::Attach, true),
            (
                SelectionCause::SpawnRollback,
                SelectionSource::SpawnRollback,
                false,
            ),
            (
                SelectionCause::BackgroundCleanup,
                SelectionSource::BackgroundCleanup,
                false,
            ),
            (
                SelectionCause::LivenessReconcile,
                SelectionSource::LivenessReconcile,
                false,
            ),
        ];
        for (cause, source, user_initiated) in cases {
            assert_eq!(cause.source(), source);
            assert_eq!(cause.user_initiated(), user_initiated);
        }
        for (cause, source, user_initiated) in [
            (
                SelectionCause::SessionCreated(TrustedCreateIntent::User),
                SelectionSource::SessionCreated,
                true,
            ),
            (
                SelectionCause::SessionCreated(TrustedCreateIntent::Background),
                SelectionSource::SessionCreated,
                false,
            ),
            (
                SelectionCause::Restart(TrustedRestartIntent::User),
                SelectionSource::Restart,
                true,
            ),
            (
                SelectionCause::Restart(TrustedRestartIntent::Background),
                SelectionSource::Restart,
                false,
            ),
            (
                SelectionCause::ResourceMonitor(TrustedResourceIntent::User),
                SelectionSource::ResourceMonitor,
                true,
            ),
            (
                SelectionCause::ResourceMonitor(TrustedResourceIntent::Watchdog),
                SelectionSource::ResourceMonitor,
                false,
            ),
        ] {
            assert_eq!(cause.source(), source);
            assert_eq!(cause.user_initiated(), user_initiated);
        }
    }

    #[tokio::test]
    async fn bootstrapping_rejects_every_external_admission_without_allocating() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        let admission_before = coordinator.inner.admission.available_permits();
        let create_before = coordinator.inner.create_tickets.available_permits();

        assert_eq!(
            coordinator.snapshot().await.unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator
                .transition(SelectionRequest::user_switch(Uuid::new_v4()))
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert!(matches!(
            coordinator.reserve_auto_close(),
            Err(SelectionCoordinatorError::Busy)
        ));
        assert_eq!(
            coordinator
                .reserve_create(TrustedCreateIntent::User)
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy
        );
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(Uuid::new_v4(), 1)
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator
                .watchdog_resource_kill(Uuid::new_v4())
                .await
                .unwrap_err(),
            SelectionCoordinatorError::Busy.to_string()
        );
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            admission_before
        );
        assert_eq!(
            coordinator.inner.create_tickets.available_permits(),
            create_before
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn restore_is_fifo_first_and_queues_external_work_until_release() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(manager)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore coordinator test app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");

        let guard = coordinator
            .submit_restore_first()
            .await
            .expect("submit first restore job");
        assert_eq!(
            CoordinatorPhase::from_u8(coordinator.inner.phase.load(Ordering::Acquire)),
            CoordinatorPhase::Running
        );
        assert!(matches!(
            coordinator.submit_restore_first().await,
            Err(error) if error == SelectionCoordinatorError::Busy.to_string()
        ));
        let mut queued_snapshot = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.snapshot().await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut queued_snapshot)
                .await
                .is_err(),
            "hydration admitted after restore must remain queued behind it"
        );
        guard.finish();
        assert_eq!(
            queued_snapshot
                .await
                .expect("join queued hydration")
                .expect("queued hydration succeeds")
                .revision(),
            0
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn restore_held_route_loss_waits_then_reconciles_the_public_row() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/restore-route-loss".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore-held route-loss app");
        coordinator
            .start(app.handle().clone())
            .expect("start selection coordinator");
        let guard = coordinator.submit_restore_first().await.unwrap();

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let mut route_loss = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 71).await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut route_loss)
                .await
                .is_err(),
            "route loss must queue behind the held restore job"
        );
        assert!(!matches!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(_)
        ));

        guard.finish();
        assert_eq!(
            route_loss
                .await
                .expect("join route-loss waiter")
                .expect("route-loss reconciliation"),
            CriticalAdmissionOutcome::Completed(())
        );
        let row = manager.read().await.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(71));
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn restore_transaction_publishes_rows_before_one_final_selection() {
        use crate::config::sessions_persistence::PersistedSession;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let broadcaster = WsBroadcaster::new();
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(DetachedSessionsState::default())
            .manage(broadcaster.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build restore transaction app");
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        assert!(app.manage(pty));
        coordinator
            .start(app.handle().clone())
            .expect("start restore coordinator");
        let guard = coordinator.submit_restore_first().await.unwrap();
        let transaction = guard.transaction(app.handle().clone());
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        let first = transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: "first".to_string(),
                    shell: "shell".to_string(),
                    shell_args: Vec::new(),
                    working_directory: "C:/first".to_string(),
                    status: Some(SessionStatus::Exited(7)),
                    ..PersistedSession::default()
                },
                working_directory: "C:/first".to_string(),
                is_coordinator: false,
                is_root_agent: false,
            })
            .await
            .unwrap();
        transaction
            .restore_dormant_inline(DormantRestoreRequest {
                persisted: PersistedSession {
                    name: "second".to_string(),
                    shell: "shell".to_string(),
                    shell_args: Vec::new(),
                    working_directory: "C:/second".to_string(),
                    status: Some(SessionStatus::Exited(8)),
                    ..PersistedSession::default()
                },
                working_directory: "C:/second".to_string(),
                is_coordinator: false,
                is_root_agent: false,
            })
            .await
            .unwrap();
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_created"
        );
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_created"
        );
        assert!(events_rx.try_recv().is_err());

        let target = Uuid::parse_str(&first.id).unwrap();
        let selection = transaction
            .restore_selection_inline(Some(target))
            .await
            .unwrap()
            .expect("restore selection changes");
        assert_eq!(selection.id(), Some(target));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.status(), Some(&SessionStatus::Exited(7)));
        assert_eq!(
            events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            "session_switched"
        );
        assert!(events_rx.try_recv().is_err());
        guard.finish();
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn route_loss_publishes_destroyed_exited_row_and_one_dormant_selection() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let broadcaster = WsBroadcaster::new();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(broadcaster)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build route-loss app");
        coordinator
            .start(app.handle().clone())
            .expect("start route-loss coordinator");
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |event| {
                let _ = events_tx.send((event_name, event.payload().to_string()));
            });
        }

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let sender = coordinator.container_lifecycle_sender();
        assert_eq!(
            sender.route_lost(session.id, 44).await.unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        let row = manager.read().await.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(44));
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.status(), Some(&SessionStatus::Exited(44)));
        assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
        let observed = (0..3)
            .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec!["session_destroyed", "session_created", "session_switched"]
        );
        let revision = selection.revision();
        assert_eq!(
            sender.route_lost(session.id, 99).await.unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        assert_eq!(
            manager.read().await.selection_payload().await.revision(),
            revision
        );
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn admission_and_create_ticket_budgets_are_exact_and_fail_fast() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);

        let mut create_tickets = Vec::new();
        for _ in 0..CREATE_TICKET_CAPACITY {
            create_tickets.push(
                coordinator
                    .reserve_create(TrustedCreateIntent::Background)
                    .await
                    .expect("create ticket within budget"),
            );
        }
        assert_eq!(
            coordinator
                .reserve_create(TrustedCreateIntent::Background)
                .await
                .expect_err("seventeenth slow create must fail"),
            SelectionCoordinatorError::Busy
        );

        let mut general = Vec::new();
        for _ in 0..(COORDINATOR_QUEUE_CAPACITY - CREATE_TICKET_CAPACITY) {
            general.push(
                coordinator
                    .reserve_auto_close()
                    .expect("unparked queue capacity remains available"),
            );
        }
        assert!(matches!(
            coordinator.reserve_auto_close(),
            Err(SelectionCoordinatorError::Busy)
        ));
        assert_eq!(
            COORDINATOR_ADMISSION_CAPACITY,
            COORDINATOR_QUEUE_CAPACITY + 1
        );

        drop(general);
        drop(create_tickets);
        coordinator
            .reserve_auto_close()
            .expect("dropped reservations return capacity");
    }

    #[tokio::test]
    async fn route_loss_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::RouteLoss,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn route_loss_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::RouteLoss,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn watchdog_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::WatchdogKill,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn watchdog_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::WatchdogKill,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn background_cleanup_admission_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::BackgroundCleanup,
            CriticalWaitPoint::AdmissionPermit,
        )
        .await;
    }

    #[tokio::test]
    async fn background_cleanup_queue_slot_wait_cancellation_releases_critical_key() {
        assert_cancelled_critical_waiter_releases_key(
            CriticalAdmissionKind::BackgroundCleanup,
            CriticalWaitPoint::QueueSlot,
        )
        .await;
    }

    #[tokio::test]
    async fn full_queue_critical_route_waiter_is_deduplicated_and_runs_after_capacity_returns() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-capacity".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(LifecycleTestBackend::default());
        backend.set_live(session.id, true);
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-capacity app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let mut reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(coordinator.inner.admission.available_permits(), 0);

        backend.set_live(session.id, false);
        pty.lock()
            .unwrap()
            .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
        let mut first_waiter = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 82).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if coordinator.inner.critical_keys.lock().unwrap().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("critical waiter registers while the queue is full");
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 99)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut first_waiter)
                .await
                .is_err(),
            "critical waiter must remain pending while all logical capacity is held"
        );

        drop(reservations.pop());
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(reservations);
        guard.finish();
        assert_eq!(
            first_waiter
                .await
                .expect("join critical waiter")
                .expect("critical route-loss result"),
            CriticalAdmissionOutcome::Completed(())
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert_eq!(
            manager
                .read()
                .await
                .get_session(session.id)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(82)
        );
        assert_eq!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 100)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::Completed(())
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn full_queue_keeps_critical_kinds_distinct_and_deduplicates_each_kind() {
        use crate::pty::backend::SessionBackendKind;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/critical-kind-capacity".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            LifecycleTestBackend::default(),
        ))));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(pty)
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build critical-kind app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let reservations = (0..COORDINATOR_QUEUE_CAPACITY)
            .map(|_| coordinator.reserve_auto_close().unwrap())
            .collect::<Vec<_>>();

        let route = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(session.id, 31).await })
        };
        let watchdog = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
        };
        let cleanup = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.background_destroy(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.inner.critical_keys.lock().unwrap().len() != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all critical kinds register independently at full capacity");

        assert!(matches!(
            coordinator
                .container_lifecycle_sender()
                .route_lost(session.id, 32)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));
        assert!(matches!(
            coordinator
                .watchdog_resource_kill(session.id)
                .await
                .unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));
        assert!(matches!(
            coordinator.background_destroy(session.id).await.unwrap(),
            CriticalAdmissionOutcome::AlreadyPending
        ));

        coordinator
            .close_and_join_with_budget(Duration::from_millis(25))
            .await;
        guard.finish();
        drop(reservations);
        for error in [
            route.await.unwrap().unwrap_err(),
            watchdog.await.unwrap().unwrap_err(),
            cleanup.await.unwrap().unwrap_err(),
        ] {
            assert_eq!(error, SelectionCoordinatorError::Unavailable.to_string());
        }
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn shutdown_drain_runs_common_resource_rollback_for_finalize_and_drop_jobs() {
        use crate::pty::backend::{PtyBackend, SessionBackendKind};

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build shutdown-drain app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let manager_handle = manager.read().await.clone();

        let mut finalizer_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let finalizer_session = manager_handle
            .create_pending_session(
                &mut finalizer_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/shutdown-finalizer".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(finalizer_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(finalizer_session.id, SessionBackendKind::LocalProcess);

        let mut drop_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let drop_session = manager_handle
            .create_pending_session(
                &mut drop_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/shutdown-drop".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(drop_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(drop_session.id, SessionBackendKind::LocalProcess);

        let finalizer = tokio::spawn(async move { finalizer_ticket.finalize(Vec::new()).await });
        drop(drop_ticket);
        tokio::task::yield_now().await;
        let close = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.close_and_join().await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator.inner.shutdown.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown signal becomes visible");
        guard.finish();
        close.await.expect("join close task");

        assert_eq!(
            finalizer
                .await
                .expect("join finalizer caller")
                .expect_err("queued finalizer is unavailable during shutdown"),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(backend.kill_count(finalizer_session.id), 1);
        assert_eq!(backend.kill_count(drop_session.id), 1);
        assert!(!backend.has_session(finalizer_session.id));
        assert!(!backend.has_session(drop_session.id));
        assert!(!pty.lock().unwrap().has_session(finalizer_session.id));
        assert!(!pty.lock().unwrap().has_session(drop_session.id));
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn normal_shutdown_drain_joins_pending_container_stop_exactly_once() {
        assert_pending_container_shutdown_waits_for_stop(false).await;
    }

    #[tokio::test]
    async fn normal_shutdown_drain_joins_real_pending_container_start_and_stop_exactly_once() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            false,
            false,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test]
    async fn cancelled_and_panicked_post_spawn_creates_roll_back_without_ghost_events() {
        use crate::pty::backend::SessionBackendKind;
        use tauri::Listener;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build cancellation rollback app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        let manager_handle = manager.read().await.clone();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }

        let mut cancelled_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let cancelled_session = manager_handle
            .create_pending_session(
                &mut cancelled_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/cancelled-create".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(cancelled_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(cancelled_session.id, SessionBackendKind::LocalProcess);
        let (cancel_started, cancel_started_rx) = oneshot::channel();
        let cancelled_task = tokio::spawn(async move {
            let ticket = cancelled_ticket;
            let _ = cancel_started.send(());
            std::future::pending::<()>().await;
            drop(ticket);
        });
        cancel_started_rx.await.unwrap();
        cancelled_task.abort();
        assert!(cancelled_task.await.unwrap_err().is_cancelled());

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let aggregate = manager_handle.aggregate_snapshot().await;
                if !aggregate.pending_ids.contains(&cancelled_session.id)
                    && !pty.lock().unwrap().has_session(cancelled_session.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled create rollback completes");
        assert_eq!(backend.kill_count(cancelled_session.id), 1);

        let mut panicked_ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let panicked_session = manager_handle
            .create_pending_session(
                &mut panicked_ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/panicked-create".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(panicked_session.id, true);
        pty.lock()
            .unwrap()
            .record_route(panicked_session.id, SessionBackendKind::LocalProcess);
        let panicked_task = tokio::spawn(async move {
            let _ticket = panicked_ticket;
            panic!("synthetic create task panic");
        });
        assert!(panicked_task.await.unwrap_err().is_panic());

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let aggregate = manager_handle.aggregate_snapshot().await;
                if !aggregate.pending_ids.contains(&panicked_session.id)
                    && !pty.lock().unwrap().has_session(panicked_session.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panicked create rollback completes");
        assert_eq!(backend.kill_count(panicked_session.id), 1);
        assert!(manager_handle
            .aggregate_snapshot()
            .await
            .sessions
            .is_empty());
        assert!(events_rx.try_recv().is_err());
        assert_eq!(
            coordinator.inner.create_tickets.available_permits(),
            CREATE_TICKET_CAPACITY
        );
        assert_eq!(
            coordinator.inner.admission.available_permits(),
            COORDINATOR_ADMISSION_CAPACITY
        );
        coordinator.close_and_join().await;
    }

    #[tokio::test]
    async fn capped_shutdown_aborts_a_held_worker_then_cleans_pending_resources_and_keys() {
        use crate::pty::backend::{PtyBackend, SessionBackendKind};

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build capped shutdown app");
        coordinator.start(app.handle().clone()).unwrap();
        let guard = coordinator.submit_restore_first().await.unwrap();
        let manager_handle = manager.read().await.clone();

        let mut ticket = coordinator
            .reserve_create(TrustedCreateIntent::Background)
            .await
            .unwrap();
        let pending = manager_handle
            .create_pending_session(
                &mut ticket,
                "shell".to_string(),
                Vec::new(),
                "C:/aborted-shutdown".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        backend.set_live(pending.id, true);
        pty.lock()
            .unwrap()
            .record_route(pending.id, SessionBackendKind::LocalProcess);
        let finalizer = tokio::spawn(async move { ticket.finalize(Vec::new()).await });
        let route_waiter = {
            let sender = coordinator.container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(pending.id, 9).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while coordinator.inner.critical_keys.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("route-loss key is registered behind the held worker");

        let started = std::time::Instant::now();
        coordinator
            .close_and_join_with_budget(Duration::from_millis(25))
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "capped coordinator shutdown must not wait for the held transaction"
        );
        guard.finish();

        assert_eq!(
            finalizer.await.unwrap().unwrap_err(),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(
            route_waiter.await.unwrap().unwrap_err(),
            SelectionCoordinatorError::Unavailable.to_string()
        );
        assert_eq!(backend.kill_count(pending.id), 1);
        assert!(!backend.has_session(pending.id));
        assert!(!pty.lock().unwrap().has_session(pending.id));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager_handle
                    .aggregate_snapshot()
                    .await
                    .pending_ids
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retained pending-create rollback reaches terminal manager state");
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.pending_ids.is_empty());
        assert!(aggregate.sessions.is_empty());
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn capped_shutdown_abort_joins_pending_container_stop_exactly_once() {
        assert_pending_container_shutdown_waits_for_stop(true).await;
    }

    #[tokio::test]
    async fn capped_shutdown_abort_joins_real_pending_container_start_and_stop_exactly_once() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            false,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_worker_spawn_failure_falls_back_without_losing_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Success,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_stop_error_retains_and_retries_the_deterministic_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Error,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_real_start_stop_panic_retains_and_retries_the_deterministic_handle() {
        assert_real_pending_container_start_shutdown_waits_for_stop(
            true,
            true,
            CanceledStartStopOutcome::Panic,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_handle_cleanup_worker_spawn_failure_falls_back_without_panic() {
        use crate::pty::backend::SessionBackendKind;
        use crate::pty::container_backend::ContainerTransportBackend;
        use crate::pty::container_runtime::ContainerRuntimeHandle;

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (stop_started, stop_started_rx) = oneshot::channel();
        let runtime = Arc::new(GatedStopRuntime {
            stop_started: Mutex::new(Some(stop_started)),
            stop_calls: AtomicUsize::new(0),
            stop_hold: Duration::from_secs(30),
            active_stops: AtomicUsize::new(0),
            deadline_seen: AtomicBool::new(false),
        });
        let output_senders = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        container_backend.inject_shutdown_worker_spawn_failure_for_test();
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            Arc::new(LifecycleTestBackend::default()),
            container_backend.clone(),
        )));
        let session_id = Uuid::new_v4();
        let _transport_receiver =
            container_backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id,
                container_id: format!("container-{session_id}"),
            });
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_route(session_id, SessionBackendKind::ContainerTransport);

        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build installed-handle admission-failure app");
        coordinator
            .start(app.handle().clone())
            .expect("start installed-handle admission-failure coordinator");
        coordinator
            .submit_restore_first()
            .await
            .expect("submit installed-handle restore")
            .finish();

        let kill_pty = Arc::clone(&pty);
        let kill = tokio::task::spawn_blocking(move || {
            match catch_unwind(AssertUnwindSafe(|| {
                kill_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .kill(session_id)
            })) {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("cleanup admission failure panicked".to_string()),
            }
        });
        tokio::time::timeout(Duration::from_secs(1), stop_started_rx)
            .await
            .expect("installed-handle fallback stop starts")
            .expect("installed-handle fallback stop witness is delivered");
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 1);
        assert!(!container_backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (false, 1, 1)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);

        let close_budget = Duration::from_millis(200);
        let close_started = Instant::now();
        coordinator.close_and_join_with_budget(close_budget).await;
        assert!(
            close_started.elapsed() <= close_budget + Duration::from_millis(550),
            "installed-handle close must consume the shared deadline"
        );
        tokio::time::timeout(Duration::from_secs(1), kill)
            .await
            .expect("installed-handle fallback kill finishes after shutdown cancellation")
            .expect("join installed-handle fallback kill")
            .expect("cleanup admission failure must not panic or lose the stop");

        let manager_handle = manager.read().await.clone();
        let aggregate = manager_handle.aggregate_snapshot().await;
        assert!(aggregate.sessions.is_empty());
        assert!(aggregate.pending_ids.is_empty());
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active_stops.load(Ordering::SeqCst), 0);
        assert!(runtime.deadline_seen.load(Ordering::SeqCst));
        assert!(!container_backend.contains_transport_state_for_test(session_id));
        assert!(!pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id));
        assert_eq!(
            container_backend.shutdown_work_state_for_test(),
            (true, 0, 0)
        );
        assert_eq!(container_backend.shutdown_worker_count_for_test(), 0);
        pty.lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_all_started_containers_blocking(Duration::from_secs(1));
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            1,
            "later global sweep must not repeat the recovered stop"
        );
    }

    #[tokio::test]
    async fn joined_worker_retains_no_coordinator_or_extra_pty_owner() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(LifecycleTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let weak_inner = Arc::downgrade(&coordinator.inner);
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build retention test app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        coordinator.close_and_join().await;
        assert!(coordinator.inner.worker.lock().unwrap().is_none());

        drop(coordinator);
        assert!(weak_inner.upgrade().is_none());
        assert_eq!(Arc::strong_count(&manager), 2);
        assert_eq!(Arc::strong_count(&pty), 2);
        assert_eq!(Arc::strong_count(&backend), 2);
        drop(app);
    }

    #[tokio::test]
    async fn create_auto_selection_precondition_exists_only_for_starting_none() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let none_ticket = coordinator
            .reserve_create(TrustedCreateIntent::User)
            .await
            .expect("reserve under none");
        assert!(none_ticket.auto_select_precondition.is_some());
        drop(none_ticket);

        manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("test fixture live session");
        let live_ticket = coordinator
            .reserve_create(TrustedCreateIntent::User)
            .await
            .expect("reserve under live");
        assert!(live_ticket.auto_select_precondition.is_none());
    }

    #[tokio::test]
    async fn worker_task_local_rejects_recursive_external_submission() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        coordinator
            .inner
            .phase
            .store(CoordinatorPhase::Running as u8, Ordering::Release);
        let error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .snapshot()
                    .await
                    .expect_err("recursive snapshot must fail")
            })
            .await;
        assert_eq!(
            error,
            SelectionCoordinatorError::RecursiveSubmission.to_string()
        );

        let reserve_error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .reserve_create(TrustedCreateIntent::User)
                    .await
                    .unwrap_err()
            })
            .await;
        assert_eq!(
            reserve_error,
            SelectionCoordinatorError::RecursiveSubmission
        );
        let auto_close_error = IN_SELECTION_WORKER
            .scope((), async {
                match coordinator.reserve_auto_close() {
                    Err(error) => error,
                    Ok(_) => panic!("recursive auto-close reservation must fail"),
                }
            })
            .await;
        assert_eq!(
            auto_close_error,
            SelectionCoordinatorError::RecursiveSubmission
        );
        let route_error = IN_SELECTION_WORKER
            .scope((), async {
                coordinator
                    .container_lifecycle_sender()
                    .route_lost(Uuid::new_v4(), 1)
                    .await
                    .unwrap_err()
            })
            .await;
        assert_eq!(
            route_error,
            SelectionCoordinatorError::RecursiveSubmission.to_string()
        );
        assert!(coordinator.inner.critical_keys.lock().unwrap().is_empty());
    }

    #[test]
    fn exact_coordinator_error_strings_are_stable() {
        assert_eq!(
            SelectionCoordinatorError::Busy.to_string(),
            "selectionCoordinatorBusy"
        );
        assert_eq!(
            SelectionCoordinatorError::Unavailable.to_string(),
            "selectionCoordinatorUnavailable"
        );
        assert_eq!(
            SelectionCoordinatorError::RecursiveSubmission.to_string(),
            "selectionCoordinatorRecursiveSubmission"
        );
    }
}
