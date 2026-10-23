use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Notify};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::container_credentials::CopyOutcome;
use crate::pty::container_paths::{
    canonical_host_path_env_key, container_config_dir, host_path_env_unmappable_warning,
    ContainerEnvClass, ContainerEnvWarning, ContainerPathMap,
};
use crate::pty::container_runtime::{
    api_url_for_container, resolve_container_image, ContainerDiagnostics, ContainerRuntime,
    ContainerRuntimeControl, ContainerRuntimeHandle, ContainerStartRequest,
    RetainedContainerOwnerContext, CONTAINER_STOP_TIMEOUT, DEFAULT_CONTAINER_WORKDIR,
};
use crate::pty::container_tokens::{ContainerApiToken, ContainerApiTokenManager};
use crate::pty::context_scrape::ScreenRowsRead;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::resource_monitor::ResourceLogicalAgentSlot;
use crate::session::selection::ContainerLifecycleSender;
use crate::telegram::manager::OutputSenderMap;

pub const TRANSPORT_PROTOCOL_VERSION: u32 = 2;
pub const MAX_TRANSPORT_FRAME_BYTES: usize = crate::pty::backend::PTY_INPUT_MAX_BYTES;
const TRANSPORT_LOST_EXIT_CODE: i32 = 1;
const CLEANUP_TASK_TIMEOUT: Duration = Duration::from_secs(10);
const CONTAINER_DIAGNOSTIC_LOG_TAIL_LINES: usize = 80;
const CONTAINER_SHUTDOWN_WORKER_CAPACITY: usize = 4;
const CONTAINER_SHUTDOWN_QUEUE_CAPACITY: usize = 64;
const CONTAINER_SHUTDOWN_FALLBACK_CAPACITY: usize = 1;
const CONTAINER_SHUTDOWN_POLL: Duration = Duration::from_millis(2);
const CONTAINER_SHUTDOWN_IDLE_RECHECK: Duration = Duration::from_secs(1);
const CONTAINER_SHUTDOWN_DROP_GRACE: Duration = Duration::from_secs(1);
const CONTAINER_GLOBAL_SWEEP_RETRY_BACKOFF: Duration = Duration::from_millis(10);

// Keep this re-export so session_transport and container_backend continue to
// share exactly one normalization rule.
pub(crate) use crate::pty::container_paths::root_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteRemovalError {
    Deadline(&'static str),
    LockPoisoned(&'static str),
}

impl std::fmt::Display for RouteRemovalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deadline(owner) => {
                write!(formatter, "route removal deadline reached owner={owner}")
            }
            Self::LockPoisoned(owner) => {
                write!(formatter, "route removal lock poisoned owner={owner}")
            }
        }
    }
}

type RouteRemover = Arc<dyn Fn(Uuid, Instant) -> Result<(), RouteRemovalError> + Send + Sync>;

fn lock_mutex_until<'a, T>(mutex: &'a Mutex<T>, deadline: Instant) -> Option<MutexGuard<'a, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return None;
                }
                std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ContainerTransportTuning {
    pub ticket_ttl: Duration,
    pub handshake_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub max_idle: Duration,
    pub outbound_queue_capacity: usize,
}

impl Default for ContainerTransportTuning {
    fn default() -> Self {
        Self {
            ticket_ttl: Duration::from_secs(60),
            handshake_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(10),
            max_idle: Duration::from_secs(30),
            outbound_queue_capacity: 64,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum BridgeToHostFrame {
    Hello {
        version: u32,
        #[serde(rename = "sessionId")]
        session_id: Uuid,
        root: String,
    },
    Status {
        version: u32,
        status: Option<String>,
    },
    Exit {
        version: u32,
        code: i32,
    },
    Pong {
        version: u32,
    },
}

impl BridgeToHostFrame {
    pub(crate) fn version(&self) -> u32 {
        match self {
            BridgeToHostFrame::Hello { version, .. }
            | BridgeToHostFrame::Status { version, .. }
            | BridgeToHostFrame::Exit { version, .. }
            | BridgeToHostFrame::Pong { version } => *version,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum HostToBridgeTextFrame {
    Resize { version: u32, cols: u16, rows: u16 },
    Terminate { version: u32 },
    Ping { version: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostToBridgeFrame {
    Text(HostToBridgeTextFrame),
    Binary(Vec<u8>),
}

struct PendingSession {
    root_key: String,
    ticket_hash: String,
    ticket_expires_at: Instant,
    output_target: PtyOutputTarget,
    idle_tuning: crate::session::profile::IdleTuning,
    rows: u16,
    cols: u16,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    credential_binding: Option<ContainerCredentialBinding>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    attach_notify: Option<Arc<Notify>>,
    // #930 - dest of a copied host credential, threaded so every teardown funnel
    // can delete it. None when copy-in was not applicable.
    container_credential_path: Option<PathBuf>,
}

struct AttachingSession {
    root_key: String,
    output_target: PtyOutputTarget,
    idle_tuning: crate::session::profile::IdleTuning,
    rows: u16,
    cols: u16,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    credential_binding: Option<ContainerCredentialBinding>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    attach_notify: Option<Arc<Notify>>,
    container_credential_path: Option<PathBuf>,
}

struct ActiveSession {
    sender: mpsc::Sender<HostToBridgeFrame>,
    rows: u16,
    cols: u16,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    credential_binding: Option<ContainerCredentialBinding>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    container_credential_path: Option<PathBuf>,
}

enum ContainerSessionState {
    Pending(PendingSession),
    Attaching(AttachingSession),
    Active(ActiveSession),
}

impl ContainerSessionState {
    fn credential_binding(&self) -> Option<&ContainerCredentialBinding> {
        match self {
            Self::Pending(session) => session.credential_binding.as_ref(),
            Self::Attaching(session) => session.credential_binding.as_ref(),
            Self::Active(session) => session.credential_binding.as_ref(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ContainerCredentialBinding {
    pub client_id: String,
    pub credential_generation: String,
    pub bound_session_id: String,
    pub bound_root_object_id: crate::path_identity::FileObjectId,
    pub credential_token_hash: String,
}

impl std::fmt::Debug for ContainerCredentialBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerCredentialBinding")
            .field("client_id", &self.client_id)
            .field("credential_generation", &self.credential_generation)
            .field("bound_session_id", &self.bound_session_id)
            .field("bound_root_object_id", &self.bound_root_object_id)
            .field("credential_token_hash", &"[REDACTED]")
            .finish()
    }
}

struct RemovedSessionResources {
    session_id: Uuid,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    container_credential_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerShutdownReport {
    pub terminal: bool,
    pub retained: Vec<String>,
}

#[derive(Default)]
struct RetainedContainerCleanupRegistry {
    next_id: AtomicU64,
    entries: Mutex<HashMap<u64, Arc<RetainedContainerCleanup>>>,
}

struct RetainedContainerCleanup {
    id: u64,
    session_id: Uuid,
    reason: &'static str,
    runtime: Option<Arc<dyn ContainerRuntime>>,
    runtime_handle: Option<ContainerRuntimeHandle>,
    route_removal: Mutex<RetainedRouteRemoval>,
    non_runtime: Mutex<Option<RetainedNonRuntimeCleanup>>,
    attempt: Mutex<RetainedContainerCleanupAttempt>,
}

enum RetainedRouteRemoval {
    NotRequired,
    Pending(RouteRemover),
    Unavailable(String),
}

struct RetainedNonRuntimeCleanup {
    token_manager: Option<ContainerApiTokenManager>,
    api_client_id: Option<String>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    container_credential_path: Option<PathBuf>,
}

#[derive(Default)]
struct RetainedContainerCleanupAttempt {
    in_flight: bool,
    last_epoch: Option<u64>,
    last_error: Option<String>,
}

impl RetainedContainerCleanup {
    fn require_route_removal(&self, remover: Result<Option<RouteRemover>, RouteRemovalError>) {
        let state = match remover {
            Ok(Some(remover)) => RetainedRouteRemoval::Pending(remover),
            Ok(None) => RetainedRouteRemoval::NotRequired,
            Err(error) => RetainedRouteRemoval::Unavailable(error.to_string()),
        };
        match self.route_removal.lock() {
            Ok(mut route_removal) => *route_removal = state,
            Err(error) => {
                log::error!(
                    "[container-transport] route-removal ownership lock poisoned session={} \
                        reason={} state=retained",
                    self.session_id,
                    self.reason
                );
                *error.into_inner() = RetainedRouteRemoval::Unavailable(
                    "route-removal ownership lock poisoned".to_string(),
                );
            }
        }
    }

    fn remove_route_before_cleanup(
        &self,
        control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        let remover = match self.route_removal.lock() {
            Ok(route_removal) => match &*route_removal {
                RetainedRouteRemoval::NotRequired => return Ok(()),
                RetainedRouteRemoval::Pending(remover) => Arc::clone(remover),
                RetainedRouteRemoval::Unavailable(error) => {
                    return Err(AppError::Other(format!(
                        "route removal unavailable: {error}"
                    )))
                }
            },
            Err(_) => {
                return Err(AppError::Other(
                    "route-removal ownership lock poisoned".to_string(),
                ))
            }
        };
        let deadline = control.shutdown_deadline().ok_or_else(|| {
            AppError::Other("route removal missing global-sweep deadline".to_string())
        })?;
        if Instant::now() >= deadline {
            return Err(AppError::Other(
                RouteRemovalError::Deadline("globalSweep").to_string(),
            ));
        }
        remover(self.session_id, deadline)
            .map_err(|error| AppError::Other(format!("route removal failed: {error}")))?;
        match self.route_removal.lock() {
            Ok(mut route_removal) => {
                *route_removal = RetainedRouteRemoval::NotRequired;
                Ok(())
            }
            Err(_) => Err(AppError::Other(
                "route-removal ownership lock poisoned after removal".to_string(),
            )),
        }
    }
}

impl RetainedContainerCleanupRegistry {
    fn retain(
        self: &Arc<Self>,
        token_manager: Option<ContainerApiTokenManager>,
        runtime: Option<Arc<dyn ContainerRuntime>>,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) -> Arc<RetainedContainerCleanup> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(RetainedContainerCleanup {
            id,
            session_id: resources.session_id,
            reason,
            runtime,
            runtime_handle: resources.runtime_handle,
            route_removal: Mutex::new(RetainedRouteRemoval::NotRequired),
            non_runtime: Mutex::new(Some(RetainedNonRuntimeCleanup {
                token_manager,
                api_client_id: resources.api_client_id,
                logical_resource_slot: resources.logical_resource_slot,
                container_credential_path: resources.container_credential_path,
            })),
            attempt: Mutex::new(RetainedContainerCleanupAttempt::default()),
        });
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, Arc::clone(&entry));
        entry
    }

    #[cfg(test)]
    fn entries(&self) -> Vec<Arc<RetainedContainerCleanup>> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect()
    }

    fn is_empty_until(&self, deadline: Instant) -> Option<bool> {
        lock_mutex_until(&self.entries, deadline).map(|entries| entries.is_empty())
    }

    fn entries_for_sweep(
        &self,
        deadline: Instant,
        limit: usize,
    ) -> Vec<Arc<RetainedContainerCleanup>> {
        let Some(entries) = lock_mutex_until(&self.entries, deadline) else {
            return Vec::new();
        };
        let mut fresh = Vec::with_capacity(limit);
        let mut retries = Vec::with_capacity(limit);
        for entry in entries.values() {
            if Instant::now() >= deadline {
                break;
            }
            let attempt = match entry.attempt.try_lock() {
                Ok(attempt) => attempt,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => continue,
            };
            if attempt.in_flight {
                continue;
            }
            if attempt.last_error.is_none() && fresh.len() < limit {
                fresh.push(Arc::clone(entry));
            } else if retries.len() < limit {
                retries.push(Arc::clone(entry));
            }
        }
        let retry_capacity = limit.saturating_sub(fresh.len());
        fresh.extend(retries.into_iter().take(retry_capacity));
        fresh
    }

    fn contexts(&self) -> Vec<RetainedContainerOwnerContext> {
        let (entries, registry_error) = match self.entries.try_lock() {
            Ok(entries) => (entries, None),
            Err(TryLockError::Poisoned(error)) => (
                error.into_inner(),
                Some("retained cleanup registry lock poisoned".to_string()),
            ),
            Err(TryLockError::WouldBlock) => {
                return vec![RetainedContainerOwnerContext {
                    owner: "containerCleanupRegistry",
                    session_id: None,
                    reason: "diagnostic-snapshot".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "retained",
                    in_flight: true,
                    last_error: Some("retained cleanup registry lock unavailable".to_string()),
                }]
            }
        };
        let mut contexts = entries
            .values()
            .map(|entry| {
                let (in_flight, attempt_error) = match entry.attempt.try_lock() {
                    Ok(attempt) => (attempt.in_flight, attempt.last_error.clone()),
                    Err(TryLockError::Poisoned(error)) => {
                        let attempt = error.into_inner();
                        (
                            attempt.in_flight,
                            Some(
                                attempt
                                    .last_error
                                    .as_deref()
                                    .map(|last_error| {
                                        format!("{last_error}; cleanup attempt state lock poisoned")
                                    })
                                    .unwrap_or_else(|| {
                                        "cleanup attempt state lock poisoned".to_string()
                                    }),
                            ),
                        )
                    }
                    Err(TryLockError::WouldBlock) => (
                        true,
                        Some("cleanup attempt state lock unavailable".to_string()),
                    ),
                };
                let route_error = match entry.route_removal.try_lock() {
                    Ok(route_removal) => match &*route_removal {
                        RetainedRouteRemoval::Unavailable(error) => {
                            Some(format!("route removal unavailable: {error}"))
                        }
                        RetainedRouteRemoval::NotRequired | RetainedRouteRemoval::Pending(_) => {
                            None
                        }
                    },
                    Err(TryLockError::Poisoned(_)) => {
                        Some("route-removal ownership lock poisoned".to_string())
                    }
                    Err(TryLockError::WouldBlock) => {
                        Some("route-removal ownership lock unavailable".to_string())
                    }
                };
                let last_error = match (attempt_error, route_error) {
                    (Some(attempt), Some(route)) => Some(format!("{attempt}; {route}")),
                    (Some(error), None) | (None, Some(error)) => Some(error),
                    (None, None) => None,
                };
                RetainedContainerOwnerContext {
                    owner: "containerCleanup",
                    session_id: Some(entry.session_id),
                    reason: entry.reason.to_string(),
                    program: None,
                    runtime_handle: Some(entry.runtime_handle.is_some()),
                    state: if in_flight { "inFlight" } else { "retained" },
                    in_flight,
                    last_error,
                }
            })
            .collect::<Vec<_>>();
        if let Some(last_error) = registry_error {
            contexts.push(RetainedContainerOwnerContext {
                owner: "containerCleanupRegistry",
                session_id: None,
                reason: "diagnostic-snapshot".to_string(),
                program: None,
                runtime_handle: None,
                state: "retained",
                in_flight: false,
                last_error: Some(last_error),
            });
        }
        contexts
    }

    fn attempt(
        self: &Arc<Self>,
        entry: Arc<RetainedContainerCleanup>,
        epoch: u64,
        control: &ContainerRuntimeControl,
    ) {
        {
            let mut attempt = entry
                .attempt
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if attempt.in_flight || attempt.last_epoch == Some(epoch) {
                return;
            }
            attempt.in_flight = true;
            attempt.last_epoch = Some(epoch);
        }

        let result = catch_unwind(AssertUnwindSafe(|| {
            entry.remove_route_before_cleanup(control)?;
            if let Some(non_runtime) = entry
                .non_runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                if let Some(path) = non_runtime.container_credential_path.as_deref() {
                    crate::pty::container_credentials::remove_copied(path);
                }
                if let (Some(manager), Some(client_id)) =
                    (non_runtime.token_manager, non_runtime.api_client_id)
                {
                    manager.revoke(&client_id);
                }
                drop(non_runtime.logical_resource_slot);
            }

            match (&entry.runtime, &entry.runtime_handle) {
                (Some(runtime), Some(handle)) => {
                    runtime.stop(handle, CONTAINER_STOP_TIMEOUT, control)
                }
                (None, Some(_)) => Err(AppError::Other(
                    "container runtime handle retained without a configured runtime".to_string(),
                )),
                (_, None) => Ok(()),
            }
        }));

        let terminal = match result {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                let message = error.to_string();
                log::warn!(
                    "[container-transport] retained cleanup failed session={} reason={} \
                        state=retained error={}",
                    entry.session_id,
                    entry.reason,
                    crate::pty::container_runtime::redact_container_diagnostic_text(&message)
                );
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                attempt.in_flight = false;
                attempt.last_error = Some(message);
                false
            }
            Err(_) => {
                log::error!(
                    "[container-transport] retained cleanup panicked session={} reason={} \
                        state=retained",
                    entry.session_id,
                    entry.reason
                );
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                attempt.in_flight = false;
                attempt.last_error = Some("cleanup panicked".to_string());
                false
            }
        };

        if terminal {
            {
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                attempt.in_flight = false;
                attempt.last_error = None;
            }
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if entries
                .get(&entry.id)
                .is_some_and(|owned| Arc::ptr_eq(owned, &entry))
            {
                entries.remove(&entry.id);
            }
            log::debug!(
                "[container-transport] retained cleanup terminal session={} reason={}",
                entry.session_id,
                entry.reason
            );
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ContainerShutdownPhase {
    #[default]
    Accepting,
    Draining,
    GlobalSweep,
}

#[cfg(test)]
impl ContainerShutdownPhase {
    fn is_sealed(self) -> bool {
        self != Self::Accepting
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerShutdownWorkProvenance {
    PreSealProducer,
    GlobalSweep,
}

impl ContainerShutdownWorkProvenance {
    fn as_str(self) -> &'static str {
        match self {
            Self::PreSealProducer => "preSealProducer",
            Self::GlobalSweep => "globalSweep",
        }
    }
}

#[derive(Default)]
struct ContainerShutdownWorkState {
    phase: ContainerShutdownPhase,
    active_producers: usize,
    queued: VecDeque<ContainerShutdownWork>,
    retained: VecDeque<ContainerShutdownWork>,
    active: HashMap<u64, ContainerShutdownWorkContext>,
    active_fallbacks: usize,
    next_task_id: u64,
    terminating: bool,
    worker_count: usize,
    sweep_deadline: Option<Instant>,
    #[cfg(test)]
    fail_worker_spawn: bool,
}

struct ContainerShutdownWorkRegistry {
    shared: Arc<ContainerShutdownWorkShared>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

struct ContainerShutdownProducer {
    registry: Arc<ContainerShutdownWorkRegistry>,
}

struct ContainerShutdownWorkShared {
    state: Mutex<ContainerShutdownWorkState>,
    state_changed: Condvar,
    work_available: Condvar,
    control: ContainerRuntimeControl,
}

#[derive(Clone)]
struct ContainerShutdownWorkContext {
    session_id: Option<Uuid>,
    reason: &'static str,
    provenance: ContainerShutdownWorkProvenance,
}

struct ContainerShutdownWork {
    id: u64,
    context: ContainerShutdownWorkContext,
    control: ContainerRuntimeControl,
    run: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
}

impl Default for ContainerShutdownWorkRegistry {
    fn default() -> Self {
        Self {
            shared: Arc::new(ContainerShutdownWorkShared {
                state: Mutex::new(ContainerShutdownWorkState::default()),
                state_changed: Condvar::new(),
                work_available: Condvar::new(),
                control: ContainerRuntimeControl::default(),
            }),
            workers: Mutex::new(Vec::new()),
        }
    }
}

fn decrement_shutdown_worker_count(
    state: &mut ContainerShutdownWorkState,
    shared: &ContainerShutdownWorkShared,
) {
    if state.worker_count == 0 {
        log::error!("[container-transport] shutdown worker count underflow");
    } else {
        state.worker_count -= 1;
    }
    shared.state_changed.notify_all();
}

impl ContainerShutdownWorkRegistry {
    fn register_producer(self: &Arc<Self>) -> Option<ContainerShutdownProducer> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.phase != ContainerShutdownPhase::Accepting {
            return None;
        }
        state.active_producers += 1;
        Some(ContainerShutdownProducer {
            registry: Arc::clone(self),
        })
    }

    fn spawn_producer_owned<F>(&self, session_id: Option<Uuid>, reason: &'static str, work: F)
    where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        let control = self.shared.control.clone();
        let context = ContainerShutdownWorkContext {
            session_id,
            reason,
            provenance: ContainerShutdownWorkProvenance::PreSealProducer,
        };
        let mut work = Some(Box::new(work) as Box<dyn FnOnce(&ContainerRuntimeControl) + Send>);
        let worker_count = self.ensure_workers(&context);
        let queued = if worker_count == 0 {
            false
        } else {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.terminating || state.queued.len() >= CONTAINER_SHUTDOWN_QUEUE_CAPACITY {
                false
            } else {
                let id = state.next_task_id;
                state.next_task_id = state.next_task_id.wrapping_add(1);
                match work.take() {
                    Some(run) => {
                        state.queued.push_back(ContainerShutdownWork {
                            id,
                            context: context.clone(),
                            control: control.clone(),
                            run,
                        });
                        self.shared.work_available.notify_one();
                        true
                    }
                    None => {
                        log::error!(
                            "[container-transport] shutdown work ownership missing before \
                                admission session={} reason={}",
                            context
                                .session_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                            context.reason
                        );
                        false
                    }
                }
            }
        };
        if queued {
            return;
        }

        log::warn!(
            "[container-transport] drain-owned shutdown work admission unavailable; using \
                serialized cooperative fallback session={} reason={} workers={} queueCapacity={}",
            context
                .session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            context.reason,
            worker_count,
            CONTAINER_SHUTDOWN_QUEUE_CAPACITY
        );
        if let Some(work) = work.take() {
            self.run_producer_fallback(context, control, work, worker_count);
        }
    }

    fn run_producer_fallback(
        &self,
        context: ContainerShutdownWorkContext,
        control: ContainerRuntimeControl,
        work: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
        worker_count: usize,
    ) {
        let mut work = Some(work);
        let id = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                let deadline = control.shutdown_deadline();
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let id = state.next_task_id;
                    state.next_task_id = state.next_task_id.wrapping_add(1);
                    if let Some(run) = work.take() {
                        state.retained.push_back(ContainerShutdownWork {
                            id,
                            context: context.clone(),
                            control: control.clone(),
                            run,
                        });
                    }
                    self.shared.work_available.notify_one();
                    self.shared.state_changed.notify_all();
                    log::error!(
                        "[container-transport] drain-owned cooperative fallback retained at the \
                            absolute deadline session={} reason={} workers={} state=retained",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason,
                        worker_count
                    );
                    return;
                }
                if state.active_fallbacks < CONTAINER_SHUTDOWN_FALLBACK_CAPACITY {
                    let id = state.next_task_id;
                    state.next_task_id = state.next_task_id.wrapping_add(1);
                    state.active.insert(id, context.clone());
                    state.active_fallbacks += 1;
                    break id;
                }
                state = if let Some(deadline) = deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match self.shared.state_changed.wait_timeout(state, remaining) {
                        Ok((state, _)) => state,
                        Err(error) => error.into_inner().0,
                    }
                } else {
                    self.shared
                        .state_changed
                        .wait(state)
                        .unwrap_or_else(|error| error.into_inner())
                };
            }
        };
        if let Some(work) = work.take() {
            run_container_shutdown_work(work, &control, &context);
        }
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active.remove(&id);
        if state.active_fallbacks == 0 {
            log::error!(
                "[container-transport] synchronous shutdown work count underflow session={} \
                    reason={}",
                context
                    .session_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                context.reason
            );
        } else {
            state.active_fallbacks -= 1;
        }
        self.shared.state_changed.notify_all();
    }

    fn start_global_sweep_epoch(&self, deadline: Instant) -> bool {
        if Instant::now() >= deadline {
            return false;
        }
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            return false;
        };
        state.phase = ContainerShutdownPhase::GlobalSweep;
        state.terminating = false;
        state.sweep_deadline = Some(deadline);
        drop(state);
        self.shared.work_available.notify_all();
        let context = ContainerShutdownWorkContext {
            session_id: None,
            reason: "global-sweep-epoch",
            provenance: ContainerShutdownWorkProvenance::GlobalSweep,
        };
        self.ensure_workers(&context) > 0
    }

    fn has_owned_work_until(&self, deadline: Instant) -> Option<bool> {
        lock_mutex_until(&self.shared.state, deadline).map(|state| {
            state.active_producers > 0
                || !state.queued.is_empty()
                || !state.retained.is_empty()
                || !state.active.is_empty()
                || state.worker_count > 0
        })
    }

    fn spawn_global_sweep_owned_with_control<F>(
        &self,
        session_id: Option<Uuid>,
        reason: &'static str,
        control: ContainerRuntimeControl,
        deadline: Instant,
        work: F,
    ) -> bool
    where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        if Instant::now() >= deadline {
            return false;
        }
        let context = ContainerShutdownWorkContext {
            session_id,
            reason,
            provenance: ContainerShutdownWorkProvenance::GlobalSweep,
        };
        if self.ensure_workers(&context) == 0 {
            return false;
        }
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            return false;
        };
        if state.phase != ContainerShutdownPhase::GlobalSweep
            || state.terminating
            || state.queued.len() >= CONTAINER_SHUTDOWN_QUEUE_CAPACITY
            || Instant::now() >= deadline
        {
            return false;
        }
        let id = state.next_task_id;
        state.next_task_id = state.next_task_id.wrapping_add(1);
        state.queued.push_back(ContainerShutdownWork {
            id,
            context,
            control,
            run: Box::new(work),
        });
        self.shared.work_available.notify_one();
        true
    }

    fn ensure_workers(&self, context: &ContainerShutdownWorkContext) -> usize {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut retained = Vec::with_capacity(workers.len());
        for worker in workers.drain(..) {
            if worker.is_finished() {
                if worker.join().is_err() {
                    {
                        let mut state = self
                            .shared
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        decrement_shutdown_worker_count(&mut state, &self.shared);
                    }
                    log::error!(
                        "[container-transport] shutdown worker exited after panic session={} \
                            reason={}",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason
                    );
                }
            } else {
                retained.push(worker);
            }
        }
        *workers = retained;

        let deficit = {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.terminating {
                0
            } else {
                CONTAINER_SHUTDOWN_WORKER_CAPACITY.saturating_sub(state.worker_count)
            }
        };
        for _ in 0..deficit {
            #[cfg(test)]
            if self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .fail_worker_spawn
            {
                log::warn!(
                    "[container-transport] injected shutdown worker spawn failure session={} \
                        reason={}",
                    context
                        .session_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    context.reason
                );
                break;
            }

            let shared = Arc::clone(&self.shared);
            let worker_index = workers.len();
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.worker_count += 1;
            }
            match std::thread::Builder::new()
                .name(format!("ac-container-shutdown-{worker_index}"))
                .spawn(move || container_shutdown_worker(shared))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    {
                        let mut state = self
                            .shared
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        decrement_shutdown_worker_count(&mut state, &self.shared);
                    }
                    log::warn!(
                        "[container-transport] shutdown worker spawn failed session={} reason={} \
                            worker={} error={}",
                        context
                            .session_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        context.reason,
                        worker_index,
                        error
                    );
                    break;
                }
            }
        }
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .worker_count
    }

    fn begin_shutdown(&self, deadline: Instant) -> bool {
        self.shared.control.request_shutdown(deadline);
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            self.shared.work_available.notify_all();
            self.shared.state_changed.notify_all();
            log::error!(
                "[container-transport] shutdown state lock reached absolute deadline \
                    reason=shutdown-begin state=retained"
            );
            return false;
        };
        if state.phase == ContainerShutdownPhase::Accepting {
            state.phase = ContainerShutdownPhase::Draining;
        }
        self.shared.work_available.notify_all();
        self.shared.state_changed.notify_all();
        true
    }

    fn seal_and_drain_until(&self, deadline: Instant) -> ContainerShutdownReport {
        if !self.begin_shutdown(deadline) {
            return ContainerShutdownReport {
                terminal: false,
                retained: vec!["reason=shutdown-begin state=retained".to_string()],
            };
        }
        let mut drained = false;
        loop {
            let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
                break;
            };
            let work_drained = state.active_producers == 0
                && state.queued.is_empty()
                && state.retained.is_empty()
                && state.active.is_empty();
            if work_drained {
                state.terminating = true;
                self.shared.work_available.notify_all();
            }
            drained = work_drained && state.worker_count == 0;
            drop(state);
            if drained {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
        }

        let mut worker_lock_retained = false;
        if let Ok(mut workers) = self.workers.try_lock() {
            let mut unfinished = Vec::new();
            for worker in workers.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        {
                            let mut state = self
                                .shared
                                .state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            decrement_shutdown_worker_count(&mut state, &self.shared);
                        }
                        log::error!(
                            "[container-transport] shutdown worker panicked during bounded join"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *workers = unfinished;
        } else {
            worker_lock_retained = true;
        }

        let state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let mut retained =
                    vec!["reason=shutdown-state-snapshot state=retained".to_string()];
                if worker_lock_retained {
                    retained.push("workers=unknown state=retained".to_string());
                }
                log::error!(
                    "[container-transport] shared shutdown deadline retained ownership because the \
                        state snapshot lock is unavailable"
                );
                return ContainerShutdownReport {
                    terminal: false,
                    retained,
                };
            }
        };
        let mut retained = state
            .active
            .values()
            .chain(state.queued.iter().map(|work| &work.context))
            .chain(state.retained.iter().map(|work| &work.context))
            .map(|context| {
                format!(
                    "session={} reason={} provenance={} state=retained",
                    context
                        .session_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    context.reason,
                    context.provenance.as_str()
                )
            })
            .collect::<Vec<_>>();
        if state.active_producers > 0 {
            retained.push(format!(
                "producers={} state=retained",
                state.active_producers
            ));
        }
        if state.worker_count > 0 || worker_lock_retained {
            retained.push(format!("workers={} state=retained", state.worker_count));
        }
        let terminal = drained && retained.is_empty();
        if !terminal {
            log::error!(
                "[container-transport] shared shutdown deadline retained ownership producers={} \
                    queued={} retained={} active={} workers={} work=[{}]",
                state.active_producers,
                state.queued.len(),
                state.retained.len(),
                state.active.len(),
                state.worker_count,
                retained.join(", ")
            );
        }
        ContainerShutdownReport { terminal, retained }
    }

    #[cfg(test)]
    fn inject_worker_spawn_failure(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fail_worker_spawn = true;
    }

    #[cfg(test)]
    fn clear_worker_spawn_failure(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fail_worker_spawn = false;
    }

    #[cfg(test)]
    fn worker_count(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .worker_count
    }

    #[cfg(test)]
    fn snapshot(&self) -> (bool, usize, usize) {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (
            state.phase.is_sealed(),
            state.active_producers,
            state.queued.len() + state.retained.len() + state.active.len(),
        )
    }

    #[cfg(test)]
    fn observation(&self) -> (bool, usize, usize, usize) {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (
            state.phase.is_sealed(),
            state.active_producers,
            state.queued.len() + state.retained.len() + state.active.len(),
            state.worker_count,
        )
    }
}

static PROCESS_RETAINED_CONTAINER_WORKERS: OnceLock<Mutex<Vec<std::thread::JoinHandle<()>>>> =
    OnceLock::new();

impl ContainerShutdownWorkRegistry {
    fn join_idle_workers_until(&self, grace: Instant) -> Vec<std::thread::JoinHandle<()>> {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            let mut unfinished = Vec::with_capacity(workers.len());
            for worker in workers.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        {
                            let mut state = self
                                .shared
                                .state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            decrement_shutdown_worker_count(&mut state, &self.shared);
                        }
                        log::error!(
                            "[container-transport] shutdown worker panicked during registry drop"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *workers = unfinished;
            let busy = match lock_mutex_until(&self.shared.state, grace) {
                Some(state) => state.active.len().saturating_sub(state.active_fallbacks),
                None => break,
            };
            if workers.len() <= busy {
                break;
            }
            let remaining = grace.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
        }
        std::mem::take(&mut *workers)
    }
}

impl Drop for ContainerShutdownWorkRegistry {
    fn drop(&mut self) {
        self.shared
            .control
            .request_shutdown(Instant::now() + Duration::from_secs(1));
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.phase == ContainerShutdownPhase::Accepting {
                state.phase = ContainerShutdownPhase::Draining;
            }
            state.terminating = true;
            self.shared.work_available.notify_all();
            self.shared.state_changed.notify_all();
        }
        let retained = PROCESS_RETAINED_CONTAINER_WORKERS.get_or_init(|| Mutex::new(Vec::new()));
        {
            let mut retained = retained.lock().unwrap_or_else(|error| error.into_inner());
            let mut unfinished = Vec::with_capacity(retained.len());
            for worker in retained.drain(..) {
                if worker.is_finished() {
                    if worker.join().is_err() {
                        log::error!(
                            "[container-transport] shutdown worker panicked during retained \
                                registry drop"
                        );
                    }
                } else {
                    unfinished.push(worker);
                }
            }
            *retained = unfinished;
        }
        let grace = Instant::now() + CONTAINER_SHUTDOWN_DROP_GRACE;
        let remaining = self.join_idle_workers_until(grace);
        let retained_count = remaining.len();
        retained
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(remaining);
        log::warn!(
            "[container-transport] registry drop retained {} unfinished shutdown workers",
            retained_count
        );
    }
}

fn container_shutdown_worker(shared: Arc<ContainerShutdownWorkShared>) {
    loop {
        let work = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            loop {
                if let Some(work) = state
                    .queued
                    .pop_front()
                    .or_else(|| state.retained.pop_front())
                {
                    state.active.insert(work.id, work.context.clone());
                    shared.state_changed.notify_all();
                    break Some(work);
                }
                if state.terminating {
                    decrement_shutdown_worker_count(&mut state, &shared);
                    break None;
                }
                let effective_deadline = if state.phase == ContainerShutdownPhase::GlobalSweep {
                    state.sweep_deadline
                } else {
                    shared.control.shutdown_deadline()
                };
                match effective_deadline {
                    Some(deadline) if Instant::now() >= deadline => {
                        if state.active_producers == 0 {
                            if state.phase == ContainerShutdownPhase::Accepting {
                                state.phase = ContainerShutdownPhase::Draining;
                            }
                            state.terminating = true;
                            shared.work_available.notify_all();
                            shared.state_changed.notify_all();
                            decrement_shutdown_worker_count(&mut state, &shared);
                            break None;
                        }
                        state = shared
                            .work_available
                            .wait_timeout(state, CONTAINER_SHUTDOWN_IDLE_RECHECK)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                    Some(deadline) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        state = shared
                            .work_available
                            .wait_timeout(state, remaining)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                    None => {
                        state = shared
                            .work_available
                            .wait_timeout(state, CONTAINER_SHUTDOWN_IDLE_RECHECK)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    }
                }
            }
        };
        let Some(work) = work else {
            return;
        };
        let id = work.id;
        run_container_shutdown_work(work.run, &work.control, &work.context);
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active.remove(&id);
        shared.state_changed.notify_all();
    }
}

fn run_container_shutdown_work(
    work: Box<dyn FnOnce(&ContainerRuntimeControl) + Send + 'static>,
    control: &ContainerRuntimeControl,
    context: &ContainerShutdownWorkContext,
) {
    if catch_unwind(AssertUnwindSafe(|| work(control))).is_err() {
        log::error!(
            "[container-transport] shutdown work panicked session={} reason={} provenance={}",
            context
                .session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            context.reason,
            context.provenance.as_str()
        );
    }
}

impl ContainerShutdownProducer {
    fn spawn_owned<F>(&self, session_id: Option<Uuid>, reason: &'static str, work: F)
    where
        F: FnOnce(&ContainerRuntimeControl) + Send + 'static,
    {
        self.registry.spawn_producer_owned(session_id, reason, work);
    }
}

impl Drop for ContainerShutdownProducer {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.active_producers == 0 {
            log::error!("[container-transport] shutdown-work producer count underflow");
            return;
        }
        state.active_producers -= 1;
        if state.active_producers == 0 {
            self.registry.shared.work_available.notify_all();
            self.registry.shared.state_changed.notify_all();
        }
    }
}

struct ContainerSpawnCancellationGuard<'a> {
    backend: &'a ContainerTransportBackend,
    session_id: Uuid,
    canceled: Arc<AtomicBool>,
    late_handle: Arc<Mutex<Option<ContainerRuntimeHandle>>>,
    shutdown_producer: Option<ContainerShutdownProducer>,
    armed: bool,
}

impl ContainerSpawnCancellationGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
        self.shutdown_producer.take();
    }

    fn cancel_without_unwind(&mut self) {
        self.canceled.store(true, Ordering::Release);
        if let Some(handle) = self
            .late_handle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            if let Some(runtime) = self.backend.runtime.clone() {
                if let Some(producer) = self.shutdown_producer.as_ref() {
                    self.backend.spawn_shutdown_owned_runtime_stop(
                        producer,
                        runtime,
                        handle,
                        "canceled-late-runtime",
                    );
                } else {
                    let resources = RemovedSessionResources {
                        session_id: handle.session_id,
                        runtime_handle: Some(handle),
                        api_client_id: None,
                        logical_resource_slot: None,
                        container_credential_path: None,
                    };
                    let entry = self.backend.cleanup_ownership.retain(
                        None,
                        Some(runtime),
                        resources,
                        "canceled-late-runtime",
                    );
                    self.backend.schedule_retained_cleanup(entry, None);
                }
            }
        }
        if let Some(resources) = self.backend.remove_session_state(self.session_id) {
            self.backend.cleanup_removed_resources_async_owned(
                resources,
                "spawn-canceled",
                self.shutdown_producer.as_ref(),
            );
        }
    }
}

impl Drop for ContainerSpawnCancellationGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if catch_unwind(AssertUnwindSafe(|| self.cancel_without_unwind())).is_err() {
            log::error!(
                "[container-transport] spawn cancellation cleanup panicked session={}",
                self.session_id
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportTicketError {
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportAttachError {
    Invalid,
}

pub struct ContainerTransportBackend {
    sessions: Arc<Mutex<HashMap<Uuid, ContainerSessionState>>>,
    fanout: SessionIoFanout,
    lifecycle_sender: Option<ContainerLifecycleSender>,
    route_remover: Arc<Mutex<Option<RouteRemover>>>,
    tuning: ContainerTransportTuning,
    runtime: Option<Arc<dyn ContainerRuntime>>,
    token_manager: Option<ContainerApiTokenManager>,
    shutdown_work: Arc<ContainerShutdownWorkRegistry>,
    cleanup_ownership: Arc<RetainedContainerCleanupRegistry>,
    cleanup_attempt_epoch: AtomicU64,
    #[cfg(test)]
    runtime_settings_override: Option<crate::config::settings::AppSettings>,
    #[cfg(test)]
    issued_tickets_for_test: Arc<Mutex<HashMap<Uuid, String>>>,
}

impl ContainerTransportBackend {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<ContainerLifecycleSender>,
    ) -> Self {
        Self::with_tuning(
            output_senders,
            idle_detector,
            ws_broadcaster,
            lifecycle_sender,
            ContainerTransportTuning::default(),
        )
    }

    pub fn with_tuning(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<ContainerLifecycleSender>,
        tuning: ContainerTransportTuning,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            fanout: SessionIoFanout::new(output_senders, idle_detector, ws_broadcaster),
            lifecycle_sender,
            route_remover: Arc::new(Mutex::new(None)),
            tuning,
            runtime: None,
            token_manager: None,
            shutdown_work: Arc::new(ContainerShutdownWorkRegistry::default()),
            cleanup_ownership: Arc::new(RetainedContainerCleanupRegistry::default()),
            cleanup_attempt_epoch: AtomicU64::new(1),
            #[cfg(test)]
            runtime_settings_override: None,
            #[cfg(test)]
            issued_tickets_for_test: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_runtime(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<ContainerLifecycleSender>,
        runtime: Arc<dyn ContainerRuntime>,
        token_manager: Option<ContainerApiTokenManager>,
    ) -> Self {
        let mut backend = Self::with_tuning(
            output_senders,
            idle_detector,
            ws_broadcaster,
            lifecycle_sender,
            ContainerTransportTuning::default(),
        );
        backend.runtime = Some(runtime);
        backend.token_manager = token_manager;
        backend
    }

    pub fn tuning(&self) -> ContainerTransportTuning {
        self.tuning
    }

    pub fn credential_binding(&self, session_id: Uuid) -> Option<ContainerCredentialBinding> {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&session_id)
            .and_then(ContainerSessionState::credential_binding)
            .cloned()
    }

    pub(crate) fn set_route_remover(&self, remover: RouteRemover) {
        match self.route_remover.lock() {
            Ok(mut route_remover) => *route_remover = Some(remover),
            Err(error) => {
                log::error!(
                    "[container-transport] route-remover registry lock poisoned during setup"
                );
                *error.into_inner() = Some(remover);
            }
        }
    }

    fn route_remover_until(
        &self,
        deadline: Instant,
    ) -> Result<Option<RouteRemover>, RouteRemovalError> {
        loop {
            match self.route_remover.try_lock() {
                Ok(route_remover) => return Ok(route_remover.clone()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(RouteRemovalError::LockPoisoned("routeRemoverRegistry"))
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(RouteRemovalError::Deadline("routeRemoverRegistry"));
                    }
                    std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
                }
            }
        }
    }

    pub(crate) fn consume_ticket(
        &self,
        session_id: Uuid,
        bound_root: &str,
        ticket: &str,
    ) -> Result<(), TransportTicketError> {
        let bound_key = root_key(bound_root);
        let ticket_hash = crate::api::auth::hash_token(ticket);
        let now = Instant::now();

        let mut sessions = self.sessions.lock().unwrap();
        let Some(state) = sessions.remove(&session_id) else {
            return Err(TransportTicketError::Invalid);
        };

        let pending = match state {
            ContainerSessionState::Pending(pending) => pending,
            other => {
                sessions.insert(session_id, other);
                return Err(TransportTicketError::Invalid);
            }
        };

        if pending.root_key != bound_key
            || pending.ticket_expires_at <= now
            || !crate::api::auth::constant_time_eq(&pending.ticket_hash, &ticket_hash)
        {
            sessions.insert(session_id, ContainerSessionState::Pending(pending));
            return Err(TransportTicketError::Invalid);
        }

        let attaching = AttachingSession {
            root_key: pending.root_key,
            output_target: pending.output_target,
            idle_tuning: pending.idle_tuning,
            rows: pending.rows,
            cols: pending.cols,
            runtime_handle: pending.runtime_handle,
            api_client_id: pending.api_client_id,
            credential_binding: pending.credential_binding,
            logical_resource_slot: pending.logical_resource_slot,
            attach_notify: pending.attach_notify,
            container_credential_path: pending.container_credential_path,
        };
        sessions.insert(session_id, ContainerSessionState::Attaching(attaching));
        Ok(())
    }

    pub(crate) fn complete_hello(
        &self,
        session_id: Uuid,
        bridge_root: &str,
        sender: mpsc::Sender<HostToBridgeFrame>,
    ) -> Result<(), TransportAttachError> {
        let bridge_key = root_key(bridge_root);
        let attach = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(state) = sessions.remove(&session_id) else {
                return Err(TransportAttachError::Invalid);
            };

            let attach = match state {
                ContainerSessionState::Attaching(attach) => attach,
                other => {
                    sessions.insert(session_id, other);
                    return Err(TransportAttachError::Invalid);
                }
            };
            if attach.root_key != bridge_key {
                sessions.insert(session_id, ContainerSessionState::Attaching(attach));
                return Err(TransportAttachError::Invalid);
            }

            attach
        };

        let AttachingSession {
            output_target,
            idle_tuning,
            rows,
            cols,
            runtime_handle,
            api_client_id,
            credential_binding,
            logical_resource_slot,
            attach_notify,
            container_credential_path,
            ..
        } = attach;
        if self
            .fanout
            .register_session(session_id, idle_tuning, rows, cols, output_target)
            .is_err()
        {
            return Err(TransportAttachError::Invalid);
        }
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows,
                    cols,
                    runtime_handle,
                    api_client_id,
                    credential_binding,
                    logical_resource_slot,
                    container_credential_path,
                }),
            );
        if let Some(notify) = attach_notify {
            notify.notify_waiters();
        }
        log::info!(
            "[container-transport] attached bridge for session {}",
            session_id
        );
        Ok(())
    }

    pub(crate) fn handle_bridge_output(
        &self,
        session_id: Uuid,
        data: Vec<u8>,
    ) -> Result<(), AppError> {
        if data.len() > MAX_TRANSPORT_FRAME_BYTES {
            return Err(AppError::PtyError(
                "container transport frame exceeds 64 KiB".to_string(),
            ));
        }

        let active = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(&session_id) {
                Some(ContainerSessionState::Active(_)) => true,
                _ => return Err(AppError::SessionNotFound(session_id.to_string())),
            }
        };
        if !active {
            return Err(AppError::SessionNotFound(session_id.to_string()));
        }
        let token = self
            .fanout
            .registration_token_for_session(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;

        let session_id_str = session_id.to_string();
        self.fanout.handle_output(&token, &session_id_str, data);
        Ok(())
    }

    pub(crate) async fn handle_bridge_exit(&self, session_id: Uuid, code: i32) {
        self.close_transport(session_id, Some(code)).await;
    }

    pub(crate) async fn handle_bridge_disconnect(&self, session_id: Uuid) {
        self.close_transport(session_id, Some(TRANSPORT_LOST_EXIT_CODE))
            .await;
    }

    pub(crate) async fn handle_handshake_failed(&self, session_id: Uuid) {
        self.close_transport(session_id, Some(TRANSPORT_LOST_EXIT_CODE))
            .await;
    }

    pub(crate) fn send_ping(&self, session_id: Uuid) -> Result<(), AppError> {
        self.send_text_frame(
            session_id,
            HostToBridgeTextFrame::Ping {
                version: TRANSPORT_PROTOCOL_VERSION,
            },
        )
    }

    fn send_text_frame(
        &self,
        session_id: Uuid,
        frame: HostToBridgeTextFrame,
    ) -> Result<(), AppError> {
        self.send_frame(session_id, HostToBridgeFrame::Text(frame))
    }

    fn send_frame(&self, session_id: Uuid, frame: HostToBridgeFrame) -> Result<(), AppError> {
        let sender = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(&session_id) {
                Some(ContainerSessionState::Active(active)) => active.sender.clone(),
                _ => return Err(AppError::SessionNotFound(session_id.to_string())),
            }
        };

        match sender.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.close_transport_from_sync(session_id, TRANSPORT_LOST_EXIT_CODE);
                Err(AppError::PtyError(
                    "container transport outbound queue full".to_string(),
                ))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.close_transport_from_sync(session_id, TRANSPORT_LOST_EXIT_CODE);
                Err(AppError::SessionNotFound(session_id.to_string()))
            }
        }
    }

    async fn close_transport(&self, session_id: Uuid, exit_code: Option<i32>) {
        let resources = self.remove_session_state(session_id);
        let Some(resources) = resources else {
            return;
        };

        self.cleanup_removed_resources_async(resources, "transport-close");
        self.remove_route(session_id);
        if let Some(code) = exit_code {
            if let Some(sender) = self.lifecycle_sender.as_ref() {
                if let Err(error) = sender.route_lost(session_id, code).await {
                    log::warn!(
                        "[container-transport] route-loss reconciliation failed session={}: {}",
                        session_id,
                        error
                    );
                }
            } else {
                log::debug!(
                    "[container-transport] no lifecycle sender installed session={}",
                    session_id
                );
            }
        }
    }

    fn close_transport_from_sync(&self, session_id: Uuid, exit_code: i32) {
        let resources = self.remove_session_state(session_id);
        let Some(resources) = resources else {
            return;
        };

        self.cleanup_removed_resources_async(resources, "transport-sync-close");
        let remover = match self.route_remover.try_lock() {
            Ok(route_remover) => route_remover.clone(),
            Err(TryLockError::Poisoned(_)) => {
                log::error!(
                    "[container-transport] deferred route-remover registry lock poisoned \
                        session={} state=retained",
                    session_id
                );
                None
            }
            Err(TryLockError::WouldBlock) => {
                log::warn!(
                    "[container-transport] deferred route-remover registry lock unavailable \
                        session={} state=retained",
                    session_id
                );
                None
            }
        };
        let lifecycle_sender = self.lifecycle_sender.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(remove) = remover {
                let deadline = Instant::now() + CLEANUP_TASK_TIMEOUT;
                match catch_unwind(AssertUnwindSafe(|| remove(session_id, deadline))) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => log::warn!(
                        "[container-transport] deferred sync route removal failed session={} \
                            error={}",
                        session_id,
                        error
                    ),
                    Err(_) => log::error!(
                        "[container-transport] deferred sync route removal panicked session={}",
                        session_id
                    ),
                }
            }
            if let Some(sender) = lifecycle_sender {
                if let Err(error) = sender.route_lost(session_id, exit_code).await {
                    log::warn!(
                        "[container-transport] deferred sync route-loss reconciliation failed \
                            session={}: {}",
                        session_id,
                        error
                    );
                }
            }
        });
    }

    fn remove_session_state(&self, session_id: Uuid) -> Option<RemovedSessionResources> {
        let removed = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        if removed.is_some() {
            self.fanout.remove_session(session_id);
        }
        removed.map(|state| resources_from_state(session_id, state))
    }

    fn remove_session_state_until(
        &self,
        session_id: Uuid,
        deadline: Instant,
    ) -> Result<Option<RemovedSessionResources>, RouteRemovalError> {
        let removed = loop {
            match self.sessions.try_lock() {
                Ok(mut sessions) => break sessions.remove(&session_id),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(RouteRemovalError::LockPoisoned("containerSessionRegistry"))
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(RouteRemovalError::Deadline("containerSessionRegistry"));
                    }
                    std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
                }
            }
        };
        if removed.is_some() {
            self.fanout.remove_session(session_id);
        }
        Ok(removed.map(|state| resources_from_state(session_id, state)))
    }

    /// #930 F1 - after copy-in, if a concurrent teardown already removed the
    /// session from the map, that teardown's `remove_copied` ran before the file
    /// physically existed (idempotent no-op), so delete the now-orphaned file
    /// here. The shared `sessions` mutex serializes this check against teardown,
    /// so every interleaving converges on a deleted file. Body is identical to
    /// the previous inline recheck; extracted only so the race is unit-testable.
    fn remove_credential_if_orphaned(
        &self,
        id: Uuid,
        plan: &crate::pty::container_credentials::ContainerCredentialPlan,
    ) {
        if !self.sessions.lock().unwrap().contains_key(&id) {
            crate::pty::container_credentials::remove_copied(&plan.dest);
        }
    }

    fn remove_route(&self, session_id: Uuid) {
        let deadline = Instant::now() + CLEANUP_TASK_TIMEOUT;
        let remover = match self.route_remover_until(deadline) {
            Ok(remover) => remover,
            Err(error) => {
                log::warn!(
                    "[container-transport] route removal unavailable session={} error={}",
                    session_id,
                    error
                );
                return;
            }
        };
        if let Some(remove) = remover {
            match catch_unwind(AssertUnwindSafe(|| remove(session_id, deadline))) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!(
                    "[container-transport] route removal failed session={} error={}",
                    session_id,
                    error
                ),
                Err(_) => log::error!(
                    "[container-transport] route removal panicked session={}",
                    session_id
                ),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_pending_session(
        &self,
        id: Uuid,
        cwd: &str,
        rows: u16,
        cols: u16,
        idle_tuning: crate::session::profile::IdleTuning,
        output_target: PtyOutputTarget,
        logical_resource_slot: Option<ResourceLogicalAgentSlot>,
        attach_notify: Option<Arc<Notify>>,
        container_credential_path: Option<PathBuf>,
    ) -> Result<String, AppError> {
        let ticket = format!("acst-{}-{}", Uuid::new_v4(), Uuid::new_v4());
        let ticket_hash = crate::api::auth::hash_token(&ticket);
        let pending = PendingSession {
            root_key: root_key(cwd),
            ticket_hash,
            ticket_expires_at: Instant::now() + self.tuning.ticket_ttl,
            output_target,
            idle_tuning,
            rows,
            cols,
            runtime_handle: None,
            api_client_id: None,
            credential_binding: None,
            logical_resource_slot,
            attach_notify,
            container_credential_path,
        };

        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(&id) {
            return Err(AppError::PtyError(format!(
                "container transport session {} already exists",
                id
            )));
        }
        sessions.insert(id, ContainerSessionState::Pending(pending));

        #[cfg(test)]
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .insert(id, ticket.clone());

        log::info!(
            "[container-transport] issued one-time attach ticket for session {}",
            id
        );
        Ok(ticket)
    }

    fn install_credential_binding(
        &self,
        session_id: Uuid,
        binding: ContainerCredentialBinding,
    ) -> Result<(), AppError> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        let client_id = binding.client_id.clone();
        match state {
            ContainerSessionState::Pending(pending) => {
                pending.api_client_id = Some(client_id);
                pending.credential_binding = Some(binding);
            }
            ContainerSessionState::Attaching(attaching) => {
                attaching.api_client_id = Some(client_id);
                attaching.credential_binding = Some(binding);
            }
            ContainerSessionState::Active(active) => {
                active.api_client_id = Some(client_id);
                active.credential_binding = Some(binding);
            }
        }
        Ok(())
    }

    fn install_runtime_handle(
        &self,
        session_id: Uuid,
        handle: ContainerRuntimeHandle,
    ) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        match state {
            ContainerSessionState::Pending(pending) => pending.runtime_handle = Some(handle),
            ContainerSessionState::Attaching(attaching) => attaching.runtime_handle = Some(handle),
            ContainerSessionState::Active(active) => active.runtime_handle = Some(handle),
        }
        Ok(())
    }

    async fn spawn_runtime_backed(&self, spec: BackendSpawnSpec) -> Result<(), AppError> {
        let runtime = self
            .runtime
            .clone()
            .ok_or_else(|| AppError::Other("container runtime is not configured".to_string()))?;
        let token_manager = self.token_manager.clone().ok_or_else(|| {
            AppError::Other("container API token manager is not configured".to_string())
        })?;
        let BackendSpawnSpec {
            id,
            agent_id: _,
            coding_agent: _,
            cmd,
            args,
            resolved_agent_host_shell: _,
            cwd,
            selected_cwd,
            cols,
            rows,
            container_image,
            configured_env,
            env_remove_keys,
            env_unset,
            extra_env: _,
            idle_tuning,
            output_target,
            resource_registration: _,
            logical_resource_slot,
            container_credential,
            container_repo_mounts,
            launch_witness,
        } = spec;

        let shutdown_producer = self.shutdown_work.register_producer().ok_or_else(|| {
            AppError::Other("container runtime start rejected during shutdown".to_string())
        })?;

        let attach_notify = Arc::new(Notify::new());
        let ticket = self.create_pending_session(
            id,
            &cwd,
            rows,
            cols,
            idle_tuning,
            output_target,
            logical_resource_slot,
            Some(attach_notify.clone()),
            container_credential.as_ref().map(|p| p.dest.clone()),
        )?;
        let canceled = Arc::new(AtomicBool::new(false));
        let late_handle = Arc::new(Mutex::new(None));
        let mut cancellation_guard = ContainerSpawnCancellationGuard {
            backend: self,
            session_id: id,
            canceled: Arc::clone(&canceled),
            late_handle: Arc::clone(&late_handle),
            shutdown_producer: Some(shutdown_producer),
            armed: true,
        };

        if let Some(plan) = container_credential.as_ref() {
            match crate::pty::container_credentials::copy_in(plan) {
                // #930 - a copied token is only USED if the agent skips its
                // interactive first-run wizard: Claude gates that on
                // .claude.json flags, not on the credential file. Same gate as
                // the copy, same best-effort contract.
                Ok(CopyOutcome::Copied) => {
                    crate::pty::container_credentials::ensure_first_run_state(
                        plan,
                        DEFAULT_CONTAINER_WORKDIR,
                    )
                }
                // grinch Finding 1 - an F2 skip is NOT a copy: the dest dir or
                // leaf is a symlink/junction, so no token was written. Stamping
                // here would suppress the login wizard on a container with no
                // credential. With no token, that wizard is the correct UX.
                Ok(CopyOutcome::SkippedReparse) => log::warn!(
                    "[container-cred] copy-in skipped for session {}; not stamping first-run state",
                    id
                ),
                // Best-effort, mirror config-seed: never abort the spawn.
                Err(e) => log::warn!("[container-cred] copy-in failed for session {}: {}", id, e),
            }
            // F1 - if a concurrent teardown removed the session while we were
            // copying, its remove_copied ran before the file existed (no-op). The
            // shared std::Mutex serializes this check against teardown, so all
            // interleavings converge on a deleted file. Extracted into a method
            // so the race branch is unit-testable (see tests).
            self.remove_credential_if_orphaned(id, plan);
        }

        let token = match token_manager.mint_for_session(id, &cwd) {
            Ok(token) => token,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "mint-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
        };
        let root_identity = match crate::path_identity::verify_directory(Path::new(&cwd)) {
            Ok(identity) => identity,
            Err(_) => {
                token_manager.revoke(&token.client_id);
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "install-binding-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(AppError::Other(
                    "container credential binding root is unsafe".to_string(),
                ));
            }
        };
        let binding = ContainerCredentialBinding {
            client_id: token.client_id.clone(),
            credential_generation: token.credential_generation.clone(),
            bound_session_id: token.bound_session_id.clone(),
            bound_root_object_id: root_identity.object_id,
            credential_token_hash: token.token_hash.clone(),
        };
        if let Err(err) = self.install_credential_binding(id, binding) {
            token_manager.revoke(&token.client_id);
            if let Some(resources) = self.remove_session_state(id) {
                self.cleanup_removed_resources_offloaded_owned(
                    resources,
                    "install-binding-failure",
                    cancellation_guard.shutdown_producer.as_ref(),
                )
                .await;
            }
            return Err(err);
        }

        let request = match build_start_request(
            id,
            &cmd,
            args,
            &cwd,
            rows,
            cols,
            configured_env,
            env_remove_keys,
            env_unset,
            container_image,
            selected_cwd.as_deref(),
            ticket,
            &token,
            container_repo_mounts,
            #[cfg(test)]
            self.runtime_settings_override.as_ref(),
        ) {
            Ok(request) => request,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "build-request-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
        };
        let Some(start_producer) = cancellation_guard.shutdown_producer.as_ref() else {
            return Err(AppError::Other(
                "container runtime start lost shutdown ownership".to_string(),
            ));
        };
        let start_runtime = runtime.clone();
        let canceled_for_start = Arc::clone(&canceled);
        let late_handle_for_start = Arc::clone(&late_handle);
        let canceled_cleanup_ownership = Arc::clone(&self.cleanup_ownership);
        let canceled_cleanup_epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let (start_result_sender, start_result_receiver) = oneshot::channel();
        // #2413 - the runtime may start from here on; every later `Err` is fenced.
        launch_witness.mark();
        start_producer.spawn_owned(Some(id), "runtime-start", move |control| {
            let result = (|| {
                let handle = start_runtime.start(request, control)?;
                let canceled_handle = {
                    let mut slot = late_handle_for_start
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    *slot = Some(handle);
                    if canceled_for_start.load(Ordering::Acquire) || control.shutdown_requested() {
                        slot.take()
                    } else {
                        None
                    }
                };
                if let Some(handle) = canceled_handle {
                    let resources = RemovedSessionResources {
                        session_id: handle.session_id,
                        runtime_handle: Some(handle),
                        api_client_id: None,
                        logical_resource_slot: None,
                        container_credential_path: None,
                    };
                    let entry = canceled_cleanup_ownership.retain(
                        None,
                        Some(Arc::clone(&start_runtime)),
                        resources,
                        "canceled-late-runtime",
                    );
                    canceled_cleanup_ownership.attempt(entry, canceled_cleanup_epoch, control);
                }
                Ok::<(), AppError>(())
            })();
            if start_result_sender.send(result).is_err() {
                log::debug!(
                    "[container-transport] runtime start completed after caller cancellation"
                );
            }
        });
        let start_result = start_result_receiver.await;
        let handle = match start_result {
            Ok(Ok(())) => late_handle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .ok_or_else(|| {
                    AppError::Other("container runtime start was canceled".to_string())
                })?,
            Ok(Err(err)) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "runtime-start-failure",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(err);
            }
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "runtime-start-panic",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                }
                return Err(AppError::Other(format!(
                    "container runtime start task failed: {err}"
                )));
            }
        };
        if let Err(err) = self.install_runtime_handle(id, handle.clone()) {
            let resources = RemovedSessionResources {
                session_id: id,
                runtime_handle: Some(handle),
                api_client_id: Some(token.client_id),
                logical_resource_slot: None,
                // F3 - the session was already removed from the map before this
                // branch, so its credential deletion was initiated on that path;
                // remove_copied idempotency makes a missed second delete a no-op.
                container_credential_path: None,
            };
            self.cleanup_removed_resources_offloaded_owned(
                resources,
                "install-runtime-failure",
                cancellation_guard.shutdown_producer.as_ref(),
            )
            .await;
            return Err(err);
        }

        if self.has_session(id) {
            cancellation_guard.disarm();
            return Ok(());
        }
        match tokio::time::timeout(self.tuning.handshake_timeout, attach_notify.notified()).await {
            Ok(_) if self.has_session(id) => {
                cancellation_guard.disarm();
                Ok(())
            }
            _ => {
                if let Some(resources) = self.remove_session_state(id) {
                    let diagnostics = if let (Some(runtime), Some(handle)) =
                        (self.runtime.clone(), resources.runtime_handle.clone())
                    {
                        Some(Self::collect_container_diagnostics(runtime, handle).await)
                    } else {
                        None
                    };
                    if let Some(diagnostics) = diagnostics.as_ref() {
                        log::error!(
                            "[container-transport] attach timeout diagnostics for session {}:\n{}",
                            id,
                            diagnostics.log_summary()
                        );
                    }
                    self.cleanup_removed_resources_offloaded_owned(
                        resources,
                        "handshake-timeout",
                        cancellation_guard.shutdown_producer.as_ref(),
                    )
                    .await;
                    return Err(Self::handshake_timeout_error(
                        self.tuning.handshake_timeout,
                        diagnostics.as_ref(),
                    ));
                }
                Err(Self::handshake_timeout_error(
                    self.tuning.handshake_timeout,
                    None,
                ))
            }
        }
    }

    async fn collect_container_diagnostics(
        runtime: Arc<dyn ContainerRuntime>,
        handle: ContainerRuntimeHandle,
    ) -> ContainerDiagnostics {
        let fallback_handle = handle.clone();
        let task = tokio::task::spawn_blocking(move || {
            runtime.diagnostics(&handle, CONTAINER_DIAGNOSTIC_LOG_TAIL_LINES)
        });
        match task.await {
            Ok(diagnostics) => diagnostics,
            Err(err) => ContainerDiagnostics::unavailable(
                &fallback_handle,
                format!("container diagnostics task failed: {err}"),
            ),
        }
    }

    fn handshake_timeout_error(
        timeout: Duration,
        diagnostics: Option<&ContainerDiagnostics>,
    ) -> AppError {
        let mut message = format!("container bridge did not attach within {timeout:?}");
        if let Some(diagnostics) = diagnostics {
            message.push_str("; ");
            message.push_str(&diagnostics.ui_summary());
        }
        AppError::PtyError(message)
    }

    fn spawn_shutdown_owned_runtime_stop(
        &self,
        producer: &ContainerShutdownProducer,
        runtime: Arc<dyn ContainerRuntime>,
        handle: ContainerRuntimeHandle,
        reason: &'static str,
    ) {
        let session_id = handle.session_id;
        let resources = RemovedSessionResources {
            session_id,
            runtime_handle: Some(handle),
            api_client_id: None,
            logical_resource_slot: None,
            container_credential_path: None,
        };
        let entry = self
            .cleanup_ownership
            .retain(None, Some(runtime), resources, reason);
        self.schedule_retained_cleanup(entry, Some(producer));
        log::debug!(
            "[container-transport] tracking shutdown-owned stop session={} reason={}",
            session_id,
            reason
        );
    }

    fn retain_removed_resources(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) -> Arc<RetainedContainerCleanup> {
        self.cleanup_ownership.retain(
            self.token_manager.clone(),
            self.runtime.clone(),
            resources,
            reason,
        )
    }

    fn schedule_retained_cleanup(
        &self,
        entry: Arc<RetainedContainerCleanup>,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let ownership = Arc::clone(&self.cleanup_ownership);
        let session_id = entry.session_id;
        let reason = entry.reason;
        let work = move |control: &ContainerRuntimeControl| {
            ownership.attempt(entry, epoch, control);
        };
        if let Some(producer) = producer {
            producer.spawn_owned(Some(session_id), reason, work);
        } else if let Some(producer) = self.shutdown_work.register_producer() {
            producer.spawn_owned(Some(session_id), reason, work);
        } else {
            log::warn!(
                "[container-transport] cleanup retained for the explicit global sweep without \
                    reopening sealed shutdown work session={} reason={} state=retained",
                session_id,
                reason
            );
        }
    }

    pub(crate) fn begin_shutdown(&self, deadline: Instant) -> bool {
        self.shutdown_work.begin_shutdown(deadline)
    }

    pub(crate) fn seal_and_drain_shutdown_work_blocking(
        &self,
        deadline: Instant,
    ) -> ContainerShutdownReport {
        self.shutdown_work.seal_and_drain_until(deadline)
    }

    fn cleanup_removed_resources_async(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) {
        self.cleanup_removed_resources_async_owned(resources, reason, None);
    }

    fn cleanup_removed_resources_async_owned(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let entry = self.retain_removed_resources(resources, reason);
        self.schedule_retained_cleanup(entry, producer);
    }

    async fn cleanup_removed_resources_offloaded_owned(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let entry = self.retain_removed_resources(resources, reason);
        let (completed, completion) = oneshot::channel();
        let session_id = entry.session_id;
        let cleanup_epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let ownership = Arc::clone(&self.cleanup_ownership);
        let work = move |control: &ContainerRuntimeControl| {
            ownership.attempt(entry, cleanup_epoch, control);
            if completed.send(()).is_err() {
                log::debug!(
                    "[container-transport] {} cleanup completed after caller cancellation",
                    reason
                );
            }
        };
        if let Some(producer) = producer {
            producer.spawn_owned(Some(session_id), reason, work);
        } else if let Some(producer) = self.shutdown_work.register_producer() {
            producer.spawn_owned(Some(session_id), reason, work);
        } else {
            log::warn!(
                "[container-transport] cleanup retained for the explicit global sweep without \
                    reopening sealed shutdown work session={} reason={} state=retained",
                session_id,
                reason
            );
        }
        match tokio::time::timeout(CLEANUP_TASK_TIMEOUT, completion).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::warn!(
                    "[container-transport] {} cleanup task failed: {}",
                    reason,
                    error
                );
            }
            Err(_) => {
                log::warn!(
                    "[container-transport] {} cleanup exceeded {:?}; continuing",
                    reason,
                    CLEANUP_TASK_TIMEOUT
                );
            }
        }
    }

    pub async fn reap_expired_pending_sessions(&self) -> usize {
        let expired = {
            let sessions = self.sessions.lock().unwrap();
            let now = Instant::now();
            sessions
                .iter()
                .filter_map(|(id, state)| match state {
                    ContainerSessionState::Pending(pending) if pending.ticket_expires_at <= now => {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let mut reaped = 0;
        for session_id in expired {
            if let Some(resources) = self.remove_session_state(session_id) {
                self.cleanup_removed_resources_async(resources, "pending-reaper");
                self.remove_route(session_id);
                if let Some(sender) = self.lifecycle_sender.as_ref() {
                    if let Err(error) = sender
                        .route_lost(session_id, TRANSPORT_LOST_EXIT_CODE)
                        .await
                    {
                        log::warn!(
                            "[container-transport] pending-reaper reconciliation failed \
                                session={}: {}",
                            session_id,
                            error
                        );
                    }
                }
                reaped += 1;
            }
        }
        reaped
    }

    pub fn start_pending_reaper(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let backend = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.token().cancelled() => break,
                    _ = interval.tick() => {
                        let count = backend.reap_expired_pending_sessions().await;
                        if count > 0 {
                            log::warn!(
                                "[container-transport] reaped {} expired pending container \
                                    session(s)",
                                count
                            );
                        }
                    }
                }
            }
        });
    }

    /// #992 - Is the startup sweep load-bearing for THIS config dir, or a courtesy
    /// pass over containers another install left behind?
    ///
    /// SCOPED, and it is not a gate: the sweep runs either way. No container client
    /// on record proves only that no orphan of OURS can exist; the machine-wide
    /// label means another install's orphan still can, and we still clean it. This
    /// only decides how long we are willing to wait for the list and how loudly we
    /// complain when it fails.
    ///
    /// Fails LOAD-BEARING (no config dir, unreadable registry): an unknown answer
    /// must never downgrade the sweep.
    fn sweep_is_load_bearing(token_manager: Option<&ContainerApiTokenManager>) -> bool {
        let Some(manager) = token_manager else {
            return true;
        };
        match manager.has_container_clients() {
            Ok(has_clients) => has_clients,
            Err(err) => {
                log::warn!(
                    "[container-transport] {err}; treating the startup orphan sweep as load-bearing"
                );
                true
            }
        }
    }

    pub fn cleanup_labeled_orphans_on_startup(&self) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let token_manager = self.token_manager.clone();
        std::thread::spawn(move || {
            // #992 - the registry read happens INSIDE the thread on purpose: the
            // caller is the Tauri setup hook and must not pay a file read.
            let load_bearing = Self::sweep_is_load_bearing(token_manager.as_ref());
            // #992 - one line per start, at info, so the posture is observable. A
            // design whose correct behavior is indistinguishable from its failure
            // mode in the log is not verifiable. Do not lower this to debug.
            log::info!(
                "[container-transport] startup orphan sweep: {}",
                if load_bearing {
                    "load-bearing (this install has created containers)"
                } else {
                    "opportunistic (no container clients on record)"
                }
            );

            match runtime.cleanup_labeled_orphans(&HashSet::new(), CONTAINER_STOP_TIMEOUT) {
                Ok(report) => {
                    if !report.stopped.is_empty() {
                        log::warn!(
                            "[container-transport] stopped {} labeled orphan container(s) on \
                                startup",
                            report.stopped.len()
                        );
                    }
                    if !report.invalid_labels.is_empty() {
                        log::warn!(
                            "[container-transport] ignored {} labeled container(s) with invalid \
                                session labels",
                            report.invalid_labels.len()
                        );
                    }
                }
                Err(err) => {
                    if load_bearing {
                        log::warn!(
                            "[container-transport] startup orphan cleanup failed: {}",
                            err
                        );
                    } else {
                        // #992 - this install never created a container, so this
                        // failure means only that a courtesy pass over someone
                        // else's leftovers did not happen. That is not the user's
                        // problem and must not warn on every start: it is exactly
                        // the log noise #992 was reported from.
                        log::debug!(
                            "[container-transport] opportunistic startup orphan sweep failed: {}",
                            err
                        );
                    }
                }
            }

            if let Some(manager) = token_manager {
                match manager.revoke_all_container_clients() {
                    Ok(count) if count > 0 => {
                        log::warn!(
                            "[container-transport] revoked {} container API client(s) on startup \
                                cleanup",
                            count
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        log::warn!(
                            "[container-transport] startup container token cleanup failed: {}",
                            err
                        );
                    }
                }
            }
        });
    }

    pub fn stop_all_started_containers_blocking(
        &self,
        budget: Duration,
    ) -> ContainerShutdownReport {
        let deadline = Instant::now() + budget;
        let mut retained_residue = Vec::new();
        let session_ids = loop {
            match self.sessions.try_lock() {
                Ok(sessions) => break sessions.keys().copied().collect::<Vec<_>>(),
                Err(TryLockError::Poisoned(_)) => {
                    retained_residue.push(
                        RetainedContainerOwnerContext {
                            owner: "installedContainerRegistry",
                            session_id: None,
                            reason: "global-shutdown".to_string(),
                            program: None,
                            runtime_handle: None,
                            state: "installed",
                            in_flight: false,
                            last_error: Some(
                                "container session registry lock poisoned".to_string(),
                            ),
                        }
                        .diagnostic(),
                    );
                    break Vec::new();
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        retained_residue.push(
                            RetainedContainerOwnerContext {
                                owner: "installedContainerRegistry",
                                session_id: None,
                                reason: "global-shutdown".to_string(),
                                program: None,
                                runtime_handle: None,
                                state: "installed",
                                in_flight: true,
                                last_error: Some(
                                    "container session registry deadline reached".to_string(),
                                ),
                            }
                            .diagnostic(),
                        );
                        break Vec::new();
                    }
                    std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
                }
            }
        };
        let route_remover = if session_ids.is_empty() {
            Ok(None)
        } else {
            self.route_remover_until(deadline)
        };
        if let Err(error) = &route_remover {
            retained_residue.push(
                RetainedContainerOwnerContext {
                    owner: "routeRemoverRegistry",
                    session_id: None,
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "retained",
                    in_flight: false,
                    last_error: Some(error.to_string()),
                }
                .diagnostic(),
            );
        }
        let control = ContainerRuntimeControl::default();
        control.request_shutdown(deadline);
        for session_id in session_ids {
            if Instant::now() >= deadline {
                log::warn!(
                    "[container-transport] global shutdown cleanup budget exhausted before \
                        ownership transfer session={} state=installed",
                    session_id
                );
                break;
            }
            let resources = match self.remove_session_state_until(session_id, deadline) {
                Ok(Some(resources)) => resources,
                Ok(None) => continue,
                Err(error) => {
                    retained_residue.push(
                        RetainedContainerOwnerContext {
                            owner: "installedContainer",
                            session_id: Some(session_id),
                            reason: "global-shutdown".to_string(),
                            program: None,
                            runtime_handle: None,
                            state: "installed",
                            in_flight: false,
                            last_error: Some(error.to_string()),
                        }
                        .diagnostic(),
                    );
                    break;
                }
            };
            let entry = self.retain_removed_resources(resources, "global-shutdown-sweep");
            entry.require_route_removal(route_remover.clone());
            log::debug!(
                "[container-transport] global shutdown transferred installed handle and \
                    route-removal ownership session={} reason={} state=retained",
                entry.session_id,
                entry.reason
            );
        }

        let mut work_report = ContainerShutdownReport {
            terminal: true,
            retained: Vec::new(),
        };
        loop {
            if Instant::now() >= deadline {
                work_report.terminal = false;
                work_report
                    .retained
                    .push("reason=global-sweep-deadline state=retained".to_string());
                break;
            }
            let entries = self
                .cleanup_ownership
                .entries_for_sweep(deadline, CONTAINER_SHUTDOWN_QUEUE_CAPACITY);
            let runtime_has_retained = self
                .runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.retained_cleanup_contexts().is_empty());
            if entries.is_empty()
                && !runtime_has_retained
                && !self
                    .shutdown_work
                    .has_owned_work_until(deadline)
                    .unwrap_or(true)
            {
                break;
            }
            if !self.shutdown_work.start_global_sweep_epoch(deadline) {
                work_report.terminal = false;
                work_report
                    .retained
                    .push("reason=global-sweep-executor state=retained".to_string());
                break;
            }

            let epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
            let mut scheduled = 0_usize;
            if runtime_has_retained {
                if let Some(runtime) = self.runtime.clone() {
                    if self.shutdown_work.spawn_global_sweep_owned_with_control(
                        None,
                        "runtime-retained-cleanup",
                        control.clone(),
                        deadline,
                        move |control| runtime.retry_retained_cleanups(control),
                    ) {
                        scheduled += 1;
                    }
                }
            }
            for entry in entries {
                if Instant::now() >= deadline {
                    break;
                }
                let ownership = Arc::clone(&self.cleanup_ownership);
                let session_id = entry.session_id;
                let reason = entry.reason;
                if !self.shutdown_work.spawn_global_sweep_owned_with_control(
                    Some(session_id),
                    reason,
                    control.clone(),
                    deadline,
                    move |control| ownership.attempt(entry, epoch, control),
                ) {
                    break;
                }
                scheduled += 1;
            }

            work_report = self.shutdown_work.seal_and_drain_until(deadline);
            if !work_report.terminal || Instant::now() >= deadline {
                break;
            }
            let cleanup_remaining = !self
                .cleanup_ownership
                .is_empty_until(deadline)
                .unwrap_or(false);
            let runtime_remaining = self
                .runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.retained_cleanup_contexts().is_empty());
            if !cleanup_remaining && !runtime_remaining {
                break;
            }
            if scheduled == 0 {
                log::warn!(
                    "[container-transport] global sweep epoch made no scheduling progress \
                        state=retained"
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_GLOBAL_SWEEP_RETRY_BACKOFF.min(remaining));
        }
        let mut retained = retained_residue;
        retained.extend(work_report.retained.into_iter().map(|context| {
            if context.starts_with("owner=") {
                context
            } else {
                format!("owner=containerShutdownWork {context}")
            }
        }));
        match self.sessions.try_lock() {
            Ok(sessions) => retained.extend(sessions.iter().map(|(id, state)| {
                RetainedContainerOwnerContext {
                    owner: "installedContainer",
                    session_id: Some(*id),
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: Some(match state {
                        ContainerSessionState::Pending(pending) => pending.runtime_handle.is_some(),
                        ContainerSessionState::Attaching(attaching) => {
                            attaching.runtime_handle.is_some()
                        }
                        ContainerSessionState::Active(active) => active.runtime_handle.is_some(),
                    }),
                    state: "installed",
                    in_flight: false,
                    last_error: None,
                }
                .diagnostic()
            })),
            Err(TryLockError::Poisoned(error)) => {
                let sessions = error.into_inner();
                retained.extend(sessions.iter().map(|(id, state)| {
                    RetainedContainerOwnerContext {
                        owner: "installedContainer",
                        session_id: Some(*id),
                        reason: "global-shutdown".to_string(),
                        program: None,
                        runtime_handle: Some(match state {
                            ContainerSessionState::Pending(pending) => {
                                pending.runtime_handle.is_some()
                            }
                            ContainerSessionState::Attaching(attaching) => {
                                attaching.runtime_handle.is_some()
                            }
                            ContainerSessionState::Active(active) => {
                                active.runtime_handle.is_some()
                            }
                        }),
                        state: "installed",
                        in_flight: false,
                        last_error: Some("container session registry lock poisoned".to_string()),
                    }
                    .diagnostic()
                }));
            }
            Err(TryLockError::WouldBlock) => retained.push(
                RetainedContainerOwnerContext {
                    owner: "installedContainerRegistry",
                    session_id: None,
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "installed",
                    in_flight: true,
                    last_error: Some(
                        "container session registry diagnostic lock unavailable".to_string(),
                    ),
                }
                .diagnostic(),
            ),
        }
        retained.extend(
            self.cleanup_ownership
                .contexts()
                .into_iter()
                .map(|context| context.diagnostic()),
        );
        if let Some(runtime) = self.runtime.as_ref() {
            retained.extend(
                runtime
                    .retained_cleanup_contexts()
                    .into_iter()
                    .map(|context| context.diagnostic()),
            );
        }
        retained.sort();
        retained.dedup();
        let terminal = work_report.terminal && retained.is_empty();
        if !terminal {
            log::error!(
                "[container-transport] global shutdown cleanup retained unresolved ownership \
                    count={}",
                retained.len()
            );
        }
        ContainerShutdownReport { terminal, retained }
    }

    #[cfg(test)]
    pub(crate) fn last_issued_ticket_for_test(&self, session_id: Uuid) -> Option<String> {
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn insert_active_runtime_handle_for_test(
        &self,
        handle: ContainerRuntimeHandle,
    ) -> mpsc::Receiver<HostToBridgeFrame> {
        let session_id = handle.session_id;
        let (sender, receiver) = mpsc::channel(8);
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows: 30,
                    cols: 120,
                    runtime_handle: Some(handle),
                    api_client_id: None,
                    credential_binding: None,
                    logical_resource_slot: None,
                    container_credential_path: None,
                }),
            );
        receiver
    }

    #[cfg(test)]
    pub(crate) fn install_protocol_snapshot_session_for_test(
        &self,
        session_id: Uuid,
        binding: ContainerCredentialBinding,
        rows: u16,
        cols: u16,
        output: Vec<u8>,
    ) -> mpsc::Receiver<HostToBridgeFrame> {
        let (sender, receiver) = mpsc::channel(8);
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows,
                    cols,
                    runtime_handle: None,
                    api_client_id: Some(binding.client_id.clone()),
                    credential_binding: Some(binding),
                    logical_resource_slot: None,
                    container_credential_path: None,
                }),
            );
        let target = PtyOutputTarget::noop();
        self.fanout
            .register_session(
                session_id,
                crate::session::profile::IdleTuning::DEFAULT,
                rows,
                cols,
                target,
            )
            .expect("register protocol snapshot session");
        self.handle_bridge_output(session_id, output)
            .expect("protocol snapshot test output");
        receiver
    }

    #[cfg(test)]
    pub(crate) fn contains_transport_state_for_test(&self, session_id: Uuid) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&session_id)
    }

    #[cfg(test)]
    pub(crate) fn detached_cleanup_count_for_test(&self) -> usize {
        self.shutdown_work.snapshot().2
    }

    #[cfg(test)]
    pub(crate) fn shutdown_work_state_for_test(&self) -> (bool, usize, usize) {
        self.shutdown_work.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn shutdown_observation_for_test(&self) -> (bool, usize, usize, usize) {
        self.shutdown_work.observation()
    }

    #[cfg(test)]
    pub(crate) fn inject_shutdown_worker_spawn_failure_for_test(&self) {
        self.shutdown_work.inject_worker_spawn_failure();
    }

    #[cfg(test)]
    pub(crate) fn clear_shutdown_worker_spawn_failure_for_test(&self) {
        self.shutdown_work.clear_worker_spawn_failure();
    }

    #[cfg(test)]
    pub(crate) fn shutdown_worker_count_for_test(&self) -> usize {
        self.shutdown_work.worker_count()
    }

    #[cfg(test)]
    pub(crate) fn retained_cleanup_sessions_for_test(&self) -> Vec<Uuid> {
        let mut sessions = self
            .cleanup_ownership
            .entries()
            .into_iter()
            .map(|entry| entry.session_id)
            .collect::<Vec<_>>();
        sessions.sort();
        sessions
    }

    #[cfg(test)]
    pub(crate) fn retained_runtime_cleanup_sessions_for_test(&self) -> Vec<Uuid> {
        let mut sessions = self
            .cleanup_ownership
            .entries()
            .into_iter()
            .filter(|entry| entry.runtime_handle.is_some())
            .map(|entry| entry.session_id)
            .collect::<Vec<_>>();
        sessions.sort();
        sessions
    }

    #[cfg(test)]
    pub(crate) fn retained_cleanup_contexts_for_test(&self) -> Vec<String> {
        self.cleanup_ownership
            .contexts()
            .into_iter()
            .map(|context| context.diagnostic())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_settings_for_test(
        &mut self,
        settings: crate::config::settings::AppSettings,
    ) {
        self.runtime_settings_override = Some(settings);
    }
}

impl PtyBackend for ContainerTransportBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            if self.runtime.is_some() {
                self.spawn_runtime_backed(spec).await
            } else {
                let BackendSpawnSpec {
                    id,
                    cwd,
                    rows,
                    cols,
                    idle_tuning,
                    output_target,
                    logical_resource_slot,
                    ..
                } = spec;
                let _ticket = self.create_pending_session(
                    id,
                    &cwd,
                    rows,
                    cols,
                    idle_tuning,
                    output_target,
                    logical_resource_slot,
                    None,
                    None,
                )?;
                Ok(())
            }
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        if data.len() > MAX_TRANSPORT_FRAME_BYTES {
            return Err(AppError::PtyError(
                "container transport input exceeds 64 KiB".to_string(),
            ));
        }
        self.send_frame(id, HostToBridgeFrame::Binary(data.to_vec()))
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        self.send_text_frame(
            id,
            HostToBridgeTextFrame::Resize {
                version: TRANSPORT_PROTOCOL_VERSION,
                cols,
                rows,
            },
        )?;

        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(ContainerSessionState::Active(active)) = sessions.get_mut(&id) {
                active.cols = cols;
                active.rows = rows;
            }
        }

        self.fanout.record_resize(id);
        self.fanout.resize_screen_and_broadcast(id, cols, rows);
        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let _ = self.send_text_frame(
            id,
            HostToBridgeTextFrame::Terminate {
                version: TRANSPORT_PROTOCOL_VERSION,
            },
        );
        if let Some(resources) = self.remove_session_state(id) {
            self.cleanup_removed_resources_async(resources, "kill");
        }
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        matches!(
            self.sessions.lock().unwrap().get(&id),
            Some(ContainerSessionState::Active(_))
        )
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        self.fanout.has_rendered_visible_content(id)
    }

    fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
    ) -> Result<
        Option<crate::pty::output::PtyScreenSnapshot>,
        crate::pty::output::TerminalOutputAttachError,
    > {
        self.fanout
            .activate_terminal_output(id, label, include_history)
    }

    fn detach_terminal_output(&self, id: Uuid, label: &str) {
        self.fanout.detach_terminal_output(id, label);
    }

    fn release_window_attachments(&self, label: &str) {
        self.fanout.release_window_attachments(label);
    }

    fn shutdown_terminal_output(&self) {
        self.fanout.shutdown_terminal_output();
    }

    #[allow(private_interfaces)]
    fn copy_terminal_screen(&self, id: Uuid) -> crate::pty::backend::TerminalScreenCopyRead {
        self.fanout.copy_terminal_screen(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        // No liveness gate here, and none is needed: this backend already tears down on a
        // natural exit, and `close_transport` drops the parser before anyone could read it.
        // Parser-absent IS the container's liveness oracle.
        match self.fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            None => ScreenRowsRead::SessionOver,
        }
    }

    /// #1171 - the same read as `get_screen_rows` above, on the seam that can also say
    /// "nothing changed", and with the same oracle: parser-absent IS this backend's liveness
    /// answer, so it maps to `Gone` rather than to `Missing`.
    ///
    /// This is the whole reason `ScreenRowsSince` has four variants instead of three. The
    /// local backend, whose parser-absence is NOT conclusive, returns `Missing` from the same
    /// fanout call; keeping one mapping for both backends would have forced one of them to
    /// lie.
    fn screen_rows_since(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince {
        match self.fanout.get_screen_rows_since(id, seen) {
            ScreenRowsSince::Missing => ScreenRowsSince::Gone,
            other => other,
        }
    }

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: PathBuf,
    ) {
        self.fanout
            .register_response_watcher(session_id, request_id, response_dir);
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

pub(crate) fn parse_bridge_text_frame(text: &str) -> Result<BridgeToHostFrame, serde_json::Error> {
    serde_json::from_str(text)
}

fn resources_from_state(session_id: Uuid, state: ContainerSessionState) -> RemovedSessionResources {
    match state {
        ContainerSessionState::Pending(pending) => RemovedSessionResources {
            session_id,
            runtime_handle: pending.runtime_handle,
            api_client_id: pending.api_client_id,
            logical_resource_slot: pending.logical_resource_slot,
            container_credential_path: pending.container_credential_path,
        },
        ContainerSessionState::Attaching(attaching) => RemovedSessionResources {
            session_id,
            runtime_handle: attaching.runtime_handle,
            api_client_id: attaching.api_client_id,
            logical_resource_slot: attaching.logical_resource_slot,
            container_credential_path: attaching.container_credential_path,
        },
        ContainerSessionState::Active(active) => RemovedSessionResources {
            session_id,
            runtime_handle: active.runtime_handle,
            api_client_id: active.api_client_id,
            logical_resource_slot: active.logical_resource_slot,
            container_credential_path: active.container_credential_path,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_start_request(
    session_id: Uuid,
    cmd: &str,
    args: Vec<String>,
    cwd: &str,
    rows: u16,
    cols: u16,
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    env_unset: Vec<String>,
    container_image: Option<String>,
    selected_cwd: Option<&str>,
    registration_ticket: String,
    token: &ContainerApiToken,
    repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
    #[cfg(test)] settings_override: Option<&crate::config::settings::AppSettings>,
) -> Result<ContainerStartRequest, AppError> {
    #[cfg(not(test))]
    let settings = crate::config::settings::load_settings();
    #[cfg(test)]
    let settings = settings_override
        .cloned()
        .unwrap_or_else(crate::config::settings::load_settings);
    build_start_request_with_settings(
        session_id,
        cmd,
        args,
        cwd,
        rows,
        cols,
        configured_env,
        env_remove_keys,
        env_unset,
        container_image,
        selected_cwd,
        registration_ticket,
        token,
        &settings,
        repo_mounts,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_start_request_with_settings(
    session_id: Uuid,
    cmd: &str,
    args: Vec<String>,
    cwd: &str,
    rows: u16,
    cols: u16,
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    env_unset: Vec<String>,
    container_image: Option<String>,
    selected_cwd: Option<&str>,
    registration_ticket: String,
    token: &ContainerApiToken,
    settings: &crate::config::settings::AppSettings,
    repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
) -> Result<ContainerStartRequest, AppError> {
    if !settings.api_server_enabled {
        return Err(AppError::Other(
            "container transport requires the control-plane API server to be enabled".to_string(),
        ));
    }
    if let Some(reason) =
        crate::pty::container_paths::container_mount_source_rejection(std::path::Path::new(cwd))
    {
        let message = match selected_cwd {
            Some(selected) if selected != cwd => format!(
                "{} (selected path '{}', canonical path '{}')",
                reason, selected, cwd
            ),
            _ => reason,
        };
        return Err(AppError::Other(message));
    }
    let api_url = api_url_for_container(&settings.api_server_bind, settings.api_server_port)?;
    let child_env = sanitized_child_env(configured_env, env_remove_keys);
    match selected_cwd {
        Some(selected) if selected != cwd => log::info!(
            "[container-transport] mount source selected '{}' canonical '{}' -> '{}'",
            selected,
            cwd,
            DEFAULT_CONTAINER_WORKDIR
        ),
        _ => log::info!(
            "[container-transport] mount source '{}' -> '{}'",
            cwd,
            DEFAULT_CONTAINER_WORKDIR
        ),
    }
    for mount in &repo_mounts {
        log::info!(
            "[container-transport] repo mount '{}' -> '{}'",
            mount.host_path.display(),
            mount.container_path
        );
    }
    Ok(ContainerStartRequest {
        session_id,
        image: resolve_container_image(container_image.as_deref())?,
        host_root: cwd.to_string(),
        container_workdir: DEFAULT_CONTAINER_WORKDIR.to_string(),
        api_url,
        api_token: token.secret.clone(),
        registration_ticket,
        local_dir: crate::config::agent_local_dir_name(),
        command: cmd.to_string(),
        args,
        child_env,
        env_unset,
        cols,
        rows,
        repo_mounts,
    })
}

pub(crate) struct ContainerChildEnv {
    pub child_env: Vec<(String, String)>,
    pub env_unset: Vec<String>,
    pub warnings: Vec<ContainerEnvWarning>,
}

pub(crate) fn container_child_env(
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    map: &ContainerPathMap,
) -> ContainerChildEnv {
    let mut child_env = Vec::new();
    let mut env_unset = Vec::new();
    let mut warnings = Vec::new();
    for (key, value) in configured_env {
        if env_remove_keys
            .iter()
            .any(|remove| env_key_matches_platform(remove, &key))
            || is_reserved_container_env(&key)
        {
            continue;
        }

        let Some(canonical_key) = canonical_host_path_env_key(&key) else {
            child_env.push((key, value));
            continue;
        };

        match crate::pty::container_paths::classify_container_env(&key) {
            ContainerEnvClass::Opaque => child_env.push((key, value)),
            ContainerEnvClass::HostPathTranslate => {
                if let Some(container_value) = container_config_dir(map, &value) {
                    child_env.push((canonical_key.to_string(), container_value));
                } else {
                    let already_unset = env_unset.iter().any(|existing| existing == canonical_key);
                    if !already_unset {
                        env_unset.push(canonical_key.to_string());
                        warnings.push(host_path_env_unmappable_warning(map, canonical_key, &value));
                    }
                }
            }
        }
    }
    ContainerChildEnv {
        child_env,
        env_unset,
        warnings,
    }
}

fn env_key_matches_platform(left: &str, right: &str) -> bool {
    crate::config::settings::normalize_env_key_for_platform(left)
        == crate::config::settings::normalize_env_key_for_platform(right)
}

fn sanitized_child_env(
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
) -> Vec<(String, String)> {
    configured_env
        .into_iter()
        .filter(|(key, _)| {
            !env_remove_keys
                .iter()
                .any(|remove| remove.eq_ignore_ascii_case(key))
        })
        .filter(|(key, _)| !is_reserved_container_env(key))
        .collect()
}

fn is_reserved_container_env(key: &str) -> bool {
    key.eq_ignore_ascii_case("AGENTSCOMMANDER_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_BINARY_PATH")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_API_TOKEN")
        || key.eq_ignore_ascii_case("AGENTSCOMMANDER_ROOT")
}

#[cfg(test)]
mod tests;
