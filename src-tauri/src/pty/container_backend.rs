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
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
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
    output_target: PtyOutputTarget,
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
                    "[container-transport] route-removal ownership lock poisoned session={} reason={} state=retained",
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
                    "[container-transport] retained cleanup failed session={} reason={} state=retained error={}",
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
                    "[container-transport] retained cleanup panicked session={} reason={} state=retained",
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
                            "[container-transport] shutdown work ownership missing before admission session={} reason={}",
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
            "[container-transport] drain-owned shutdown work admission unavailable; using serialized cooperative fallback session={} reason={} workers={} queueCapacity={}",
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
                        "[container-transport] drain-owned cooperative fallback retained at the absolute deadline session={} reason={} workers={} state=retained",
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
                "[container-transport] synchronous shutdown work count underflow session={} reason={}",
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
                    log::error!(
                        "[container-transport] shutdown worker exited after panic session={} reason={}",
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

        while workers.len() < CONTAINER_SHUTDOWN_WORKER_CAPACITY {
            #[cfg(test)]
            if self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .fail_worker_spawn
            {
                log::warn!(
                    "[container-transport] injected shutdown worker spawn failure session={} reason={}",
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
            match std::thread::Builder::new()
                .name(format!("ac-container-shutdown-{worker_index}"))
                .spawn(move || container_shutdown_worker(shared))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    log::warn!(
                        "[container-transport] shutdown worker spawn failed session={} reason={} worker={} error={}",
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
        let worker_count = workers.len();
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .worker_count = worker_count;
        worker_count
    }

    fn begin_shutdown(&self, deadline: Instant) -> bool {
        self.shared.control.request_shutdown(deadline);
        let Some(mut state) = lock_mutex_until(&self.shared.state, deadline) else {
            self.shared.work_available.notify_all();
            self.shared.state_changed.notify_all();
            log::error!(
                "[container-transport] shutdown state lock reached absolute deadline reason=shutdown-begin state=retained"
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
                    "[container-transport] shared shutdown deadline retained ownership because the state snapshot lock is unavailable"
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
                "[container-transport] shared shutdown deadline retained ownership producers={} queued={} retained={} active={} workers={} work=[{}]",
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
}

static PROCESS_RETAINED_CONTAINER_WORKERS: OnceLock<Mutex<Vec<std::thread::JoinHandle<()>>>> =
    OnceLock::new();

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
        let workers = self
            .workers
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let retained = PROCESS_RETAINED_CONTAINER_WORKERS.get_or_init(|| Mutex::new(Vec::new()));
        let retained = retained.lock().unwrap_or_else(|error| error.into_inner());
        let mut retained = retained;
        for worker in workers.drain(..) {
            if worker.is_finished() {
                if worker.join().is_err() {
                    log::error!(
                        "[container-transport] shutdown worker panicked during registry drop"
                    );
                }
            } else {
                retained.push(worker);
            }
        }
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
                    break None;
                }
                state = shared
                    .work_available
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        };
        let Some(work) = work else {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.worker_count == 0 {
                log::error!("[container-transport] shutdown worker count underflow on exit");
            } else {
                state.worker_count -= 1;
            }
            shared.state_changed.notify_all();
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

            let output_target = attach.output_target.clone();
            let idle_tuning = attach.idle_tuning;
            let rows = attach.rows;
            let cols = attach.cols;
            let attach_notify = attach.attach_notify.clone();
            sessions.insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    output_target,
                    sender,
                    rows,
                    cols,
                    runtime_handle: attach.runtime_handle,
                    api_client_id: attach.api_client_id,
                    credential_binding: attach.credential_binding,
                    logical_resource_slot: attach.logical_resource_slot,
                    container_credential_path: attach.container_credential_path,
                }),
            );
            if let Some(notify) = attach_notify {
                notify.notify_waiters();
            }
            (idle_tuning, rows, cols)
        };

        self.fanout
            .register_session(session_id, attach.0, attach.1, attach.2);
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

        let output_target = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(&session_id) {
                Some(ContainerSessionState::Active(active)) => active.output_target.clone(),
                _ => return Err(AppError::SessionNotFound(session_id.to_string())),
            }
        };

        let session_id_str = session_id.to_string();
        self.fanout
            .handle_output(&output_target, session_id, &session_id_str, data);
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
                    "[container-transport] deferred route-remover registry lock poisoned session={} state=retained",
                    session_id
                );
                None
            }
            Err(TryLockError::WouldBlock) => {
                log::warn!(
                    "[container-transport] deferred route-remover registry lock unavailable session={} state=retained",
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
                        "[container-transport] deferred sync route removal failed session={} error={}",
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
                        "[container-transport] deferred sync route-loss reconciliation failed session={}: {}",
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
                "[container-transport] cleanup retained for the explicit global sweep without reopening sealed shutdown work session={} reason={} state=retained",
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
                "[container-transport] cleanup retained for the explicit global sweep without reopening sealed shutdown work session={} reason={} state=retained",
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
                            "[container-transport] pending-reaper reconciliation failed session={}: {}",
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
                                "[container-transport] reaped {} expired pending container session(s)",
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
                            "[container-transport] stopped {} labeled orphan container(s) on startup",
                            report.stopped.len()
                        );
                    }
                    if !report.invalid_labels.is_empty() {
                        log::warn!(
                            "[container-transport] ignored {} labeled container(s) with invalid session labels",
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
                            "[container-transport] revoked {} container API client(s) on startup cleanup",
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
                    "[container-transport] global shutdown cleanup budget exhausted before ownership transfer session={} state=installed",
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
                "[container-transport] global shutdown transferred installed handle and route-removal ownership session={} reason={} state=retained",
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
                    "[container-transport] global sweep epoch made no scheduling progress state=retained"
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
                "[container-transport] global shutdown cleanup retained unresolved ownership count={}",
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
                    output_target: PtyOutputTarget::noop(),
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
mod tests {
    use super::*;
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::container_paths::CLAUDE_CONFIG_DIR_KEY;
    use crate::pty::container_runtime::{ContainerCleanupReport, RETAINED_OWNER_REPORT_CAPACITY};
    use crate::pty::manager::PtyManager;
    use crate::session::manager::SessionManager;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            ContainerTransportBackend::with_tuning(
                output_senders,
                idle_detector,
                None,
                None,
                tuning,
            ),
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

    #[tokio::test]
    async fn credential_binding_is_embedded_through_pending_attach_and_active_states() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;
        let binding = ContainerCredentialBinding {
            client_id: format!("container-{id}-{}", Uuid::new_v4()),
            credential_generation: Uuid::new_v4().to_string(),
            bound_session_id: id.to_string(),
            bound_root_object_id: crate::path_identity::FileObjectId {
                volume: 7,
                file: 11,
            },
            credential_token_hash: crate::api::auth::hash_token("binding-secret-sentinel"),
        };

        backend
            .install_credential_binding(id, binding.clone())
            .expect("install binding");
        assert_eq!(backend.credential_binding(id), Some(binding.clone()));

        backend
            .consume_ticket(id, root, &ticket)
            .expect("consume ticket");
        assert_eq!(backend.credential_binding(id), Some(binding.clone()));

        let (sender, _receiver) = mpsc::channel(8);
        backend.complete_hello(id, root, sender).expect("hello");
        assert_eq!(backend.credential_binding(id), Some(binding.clone()));
        assert!(!format!("{binding:?}").contains("binding-secret-sentinel"));

        let removed = backend.remove_session_state(id).expect("remove state");
        assert_eq!(
            removed.api_client_id.as_deref(),
            Some(binding.client_id.as_str())
        );
        assert!(backend.credential_binding(id).is_none());
    }

    #[tokio::test]
    async fn expired_pending_reaper_removes_route_and_transport_state() {
        let id_root = "C:/repo/.ac/wg-1/__agent_dev";
        let tuning = ContainerTransportTuning {
            ticket_ttl: Duration::from_millis(1),
            ..ContainerTransportTuning::default()
        };
        let (backend, session_mgr) = backend_with_tuning(tuning);
        let removed = Arc::new(AtomicUsize::new(0));
        let removed_for_cb = removed.clone();
        backend.set_route_remover(Arc::new(move |_, _| {
            removed_for_cb.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let session = session_mgr
            .read()
            .await
            .create_session(
                "container".to_string(),
                Vec::new(),
                id_root.to_string(),
                Some("agent".to_string()),
                None,
                Vec::new(),
                false,
                SessionBackendKind::ContainerTransport,
            )
            .await
            .expect("session");
        backend
            .spawn(test_spec(session.id, id_root, PtyOutputTarget::noop()))
            .await
            .expect("spawn pending");

        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(backend.reap_expired_pending_sessions().await, 1);

        assert_eq!(removed.load(Ordering::SeqCst), 1);
        assert!(!backend.has_session(session.id));
    }

    #[test]
    fn kill_revokes_container_token_and_stops_runtime_handle() {
        let id = Uuid::new_v4();
        let runtime = Arc::new(RecordingRuntime::default());
        let dir = tempfile::TempDir::new().unwrap();
        let token_manager =
            ContainerApiTokenManager::new_for_path(dir.path().join("api-clients.json"));
        let token = token_manager
            .mint_for_session(id, "C:/repo/.ac/wg-1/__agent_dev")
            .expect("token");
        let (mut backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
        backend.runtime = Some(runtime.clone());
        backend.token_manager = Some(token_manager.clone());
        let (tx, _rx) = mpsc::channel(8);
        backend.sessions.lock().unwrap().insert(
            id,
            ContainerSessionState::Active(ActiveSession {
                output_target: PtyOutputTarget::noop(),
                sender: tx,
                rows: 30,
                cols: 120,
                runtime_handle: Some(ContainerRuntimeHandle {
                    session_id: id,
                    container_id: "container-id".to_string(),
                }),
                api_client_id: Some(token.client_id.clone()),
                credential_binding: None,
                logical_resource_slot: None,
                container_credential_path: None,
            }),
        );

        backend.kill(id).expect("kill");
        let report =
            backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());

        let registry = crate::api::auth::list(token_manager.path());
        assert!(
            registry
                .clients
                .iter()
                .find(|client| client.client_id == token.client_id)
                .expect("client")
                .revoked
        );
        assert_eq!(runtime.stopped(), vec![id]);
    }

    #[tokio::test]
    async fn canceled_blocking_runtime_start_stops_late_handle_and_removes_pending_state() {
        let id = Uuid::new_v4();
        let root_dir = tempfile::TempDir::new().unwrap();
        let root = root_dir.path().to_string_lossy().into_owned();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let stopped = Arc::new(Mutex::new(Vec::new()));
        let runtime = Arc::new(GatedStartRuntime {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
            stopped: Arc::clone(&stopped),
        });
        let token_dir = tempfile::TempDir::new().unwrap();
        let token_manager =
            ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
        let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning::default());
        backend.runtime = Some(runtime);
        backend.token_manager = Some(token_manager);
        backend.runtime_settings_override = Some(api_enabled_settings());
        let backend = Arc::new(backend);
        let task_backend = Arc::clone(&backend);
        let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
        spec.container_image = Some("agentscommander/test:latest".to_string());
        let mut task = tokio::spawn(async move { task_backend.spawn(spec).await });

        tokio::select! {
            result = &mut task => panic!("container spawn ended before runtime start: {result:?}"),
            started = tokio::time::timeout(Duration::from_secs(5), started_rx) => {
                started
                    .expect("runtime start reached blocking section before timeout")
                    .expect("runtime start witness sender");
            }
        }
        task.abort();
        let _ = task.await;
        release_tx.send(()).expect("release runtime start");

        for _ in 0..100 {
            if stopped.lock().unwrap().contains(&id) && !backend.has_session(id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("canceled container start leaked its pending state or late runtime handle");
    }

    #[test]
    fn global_sweep_uses_a_fresh_deadline_after_coordinator_expiry() {
        let runtime = Arc::new(SequencedStopRuntime {
            outcomes: Mutex::new(VecDeque::from([Ok(())])),
            stop_calls: AtomicUsize::new(0),
            active_deadline_seen: AtomicBool::new(false),
        });
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        );
        let session_id = Uuid::new_v4();
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
        let removed_routes = Arc::new(AtomicUsize::new(0));
        let route_count = Arc::clone(&removed_routes);
        backend.set_route_remover(Arc::new(move |_, _| {
            route_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        assert!(backend.begin_shutdown(Instant::now()));
        let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));

        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(runtime.active_deadline_seen.load(Ordering::SeqCst));
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
        assert!(!backend.contains_transport_state_for_test(session_id));

        let second = backend.stop_all_started_containers_blocking(Duration::from_millis(100));
        assert!(second.terminal, "retained={:?}", second.retained);
        assert_eq!(
            runtime.stop_calls.load(Ordering::SeqCst),
            1,
            "terminal global cleanup must not be repeated"
        );
        assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn single_production_global_sweep_retries_a_failed_handle_to_terminal() {
        let runtime = Arc::new(SequencedStopRuntime {
            outcomes: Mutex::new(VecDeque::from([Err("injected stop failure"), Ok(())])),
            stop_calls: AtomicUsize::new(0),
            active_deadline_seen: AtomicBool::new(false),
        });
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        );
        let session_id = Uuid::new_v4();
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
        let removed_routes = Arc::new(AtomicUsize::new(0));
        let route_count = Arc::clone(&removed_routes);
        backend.set_route_remover(Arc::new(move |_, _| {
            route_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 2);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());
    }

    #[test]
    fn permanently_blocked_stop_is_retained_without_exceeding_the_sweep_deadline() {
        let runtime = Arc::new(PermanentlyBlockedStopRuntime {
            stop_calls: AtomicUsize::new(0),
            active_deadline_seen: AtomicBool::new(false),
        });
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        );
        let session_id = Uuid::new_v4();
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
        let removed_routes = Arc::new(AtomicUsize::new(0));
        let route_count = Arc::clone(&removed_routes);
        backend.set_route_remover(Arc::new(move |_, _| {
            route_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let budget = Duration::from_millis(150);
        let started_at = Instant::now();
        let report = backend.stop_all_started_containers_blocking(budget);
        let elapsed = started_at.elapsed();

        assert!(!report.terminal);
        assert!(
            elapsed <= budget + Duration::from_millis(250),
            "permanently blocked stop exceeded the sweep deadline: {elapsed:?}"
        );
        assert!(runtime.active_deadline_seen.load(Ordering::SeqCst));
        assert_eq!(runtime.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(removed_routes.load(Ordering::SeqCst), 1);
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert!(
            report
                .retained
                .iter()
                .any(|context| context.contains(&session_id.to_string())),
            "retained={:?}",
            report.retained
        );
    }

    fn production_route_remover_fixture(
        runtime: Arc<RecordingRuntime>,
    ) -> (
        Arc<ContainerTransportBackend>,
        Arc<Mutex<PtyManager>>,
        Uuid,
        mpsc::Receiver<HostToBridgeFrame>,
    ) {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime,
            None,
        ));
        let local_backend: Arc<dyn PtyBackend> = backend.clone();
        let pty_manager = Arc::new(Mutex::new(PtyManager::new_for_test_with_container_backend(
            local_backend,
            Arc::clone(&backend),
        )));
        let session_id = Uuid::new_v4();
        let receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
        pty_manager
            .lock()
            .unwrap()
            .record_route(session_id, SessionBackendKind::ContainerTransport);
        crate::install_container_route_remover(&pty_manager);
        (backend, pty_manager, session_id, receiver)
    }

    fn assert_exact_production_route_residue(
        backend: &ContainerTransportBackend,
        report: &ContainerShutdownReport,
        session_id: Uuid,
        error: &str,
    ) {
        let expected = format!(
            "owner=containerCleanup session={session_id} reason=global-shutdown-sweep program=none runtimeHandle=true state=retained inFlight=false lastError=route removal failed: {error}"
        );
        assert!(report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("runtimeHandle=true")
                && (context.contains("state=retained") || context.contains("state=inFlight"))
        }));
        if report.retained.iter().any(|context| context == &expected) {
            return;
        }
        let transition_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let contexts = backend
                .cleanup_ownership
                .contexts()
                .into_iter()
                .map(|context| context.diagnostic())
                .collect::<Vec<_>>();
            if contexts.iter().any(|context| context == &expected) {
                return;
            }
            assert!(
                Instant::now() < transition_deadline,
                "expected={expected:?} report={:?} live={contexts:?}",
                report.retained
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn production_route_remover_outer_lock_deadline_retains_real_ownership() {
        let runtime = Arc::new(RecordingRuntime::default());
        let (backend, pty_manager, session_id, _receiver) =
            production_route_remover_fixture(Arc::clone(&runtime));
        let manager_guard = pty_manager.lock().unwrap();
        let budget = Duration::from_millis(150);
        let (report_sender, report_receiver) = std::sync::mpsc::channel();
        let sweep_backend = Arc::clone(&backend);
        let sweep = std::thread::spawn(move || {
            let started_at = Instant::now();
            let report = sweep_backend.stop_all_started_containers_blocking(budget);
            report_sender
                .send((started_at.elapsed(), report))
                .expect("publish real outer-lock route-removal report");
        });

        let (elapsed, report) = report_receiver
            .recv_timeout(budget + Duration::from_millis(500))
            .expect("real production route remover returns while outer mutex remains held");
        sweep.join().expect("join bounded real-lock global sweep");

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(
            elapsed <= budget + Duration::from_millis(400),
            "real outer-lock route removal exceeded the absolute deadline: {elapsed:?}"
        );
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            backend.retained_runtime_cleanup_sessions_for_test(),
            vec![session_id]
        );
        assert!(runtime.stopped().is_empty());
        assert_eq!(
            manager_guard.backend_kind(session_id),
            Some(SessionBackendKind::ContainerTransport)
        );
        assert_exact_production_route_residue(
            &backend,
            &report,
            session_id,
            "route removal deadline reached owner=ptyManager",
        );
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

        drop(manager_guard);
        let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(terminal.terminal, "retained={:?}", terminal.retained);
        assert_eq!(runtime.stopped(), vec![session_id]);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());
        assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
    }

    #[test]
    fn production_route_remover_outer_poison_is_exact_structured_residue() {
        let runtime = Arc::new(RecordingRuntime::default());
        let (backend, pty_manager, session_id, _receiver) =
            production_route_remover_fixture(Arc::clone(&runtime));
        let poison_manager = Arc::clone(&pty_manager);
        let poison = std::thread::spawn(move || {
            let _manager_guard = poison_manager.lock().unwrap();
            panic!("poison the real outer PTY manager mutex");
        });
        assert!(
            poison.join().is_err(),
            "outer mutex poison fixture must panic"
        );

        let budget = Duration::from_millis(150);
        let started_at = Instant::now();
        let report = backend.stop_all_started_containers_blocking(budget);
        let elapsed = started_at.elapsed();

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(
            elapsed <= budget + Duration::from_millis(400),
            "outer-poison route removal exceeded the absolute deadline: {elapsed:?}"
        );
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            backend.retained_runtime_cleanup_sessions_for_test(),
            vec![session_id]
        );
        assert!(runtime.stopped().is_empty());
        let poisoned_guard = match pty_manager.lock() {
            Ok(_) => panic!("outer PTY manager mutex unexpectedly recovered poison"),
            Err(error) => error.into_inner(),
        };
        assert_eq!(
            poisoned_guard.backend_kind(session_id),
            Some(SessionBackendKind::ContainerTransport)
        );
        drop(poisoned_guard);
        assert_exact_production_route_residue(
            &backend,
            &report,
            session_id,
            "route removal lock poisoned owner=ptyManager",
        );
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

        pty_manager.clear_poison();
        let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(terminal.terminal, "retained={:?}", terminal.retained);
        assert_eq!(runtime.stopped(), vec![session_id]);
        assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
    }

    #[test]
    fn production_route_remover_registry_poison_is_exact_structured_residue() {
        let runtime = Arc::new(RecordingRuntime::default());
        let (backend, pty_manager, session_id, _receiver) =
            production_route_remover_fixture(Arc::clone(&runtime));
        pty_manager.lock().unwrap().poison_route_registry_for_test();

        let budget = Duration::from_millis(150);
        let started_at = Instant::now();
        let report = backend.stop_all_started_containers_blocking(budget);
        let elapsed = started_at.elapsed();

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(
            elapsed <= budget + Duration::from_millis(400),
            "route-registry poison removal exceeded the absolute deadline: {elapsed:?}"
        );
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            backend.retained_runtime_cleanup_sessions_for_test(),
            vec![session_id]
        );
        assert!(runtime.stopped().is_empty());
        assert_eq!(
            pty_manager.lock().unwrap().backend_kind(session_id),
            Some(SessionBackendKind::ContainerTransport)
        );
        assert_exact_production_route_residue(
            &backend,
            &report,
            session_id,
            "route removal lock poisoned owner=ptyRouteRegistry",
        );
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

        pty_manager
            .lock()
            .unwrap()
            .clear_route_registry_poison_for_test();
        let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(terminal.terminal, "retained={:?}", terminal.retained);
        assert_eq!(runtime.stopped(), vec![session_id]);
        assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
    }

    #[test]
    fn production_route_remover_healthy_path_is_terminal_once() {
        let runtime = Arc::new(RecordingRuntime::default());
        let (backend, pty_manager, session_id, _receiver) =
            production_route_remover_fixture(Arc::clone(&runtime));

        let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));

        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(report.retained.is_empty());
        assert_eq!(runtime.stopped(), vec![session_id]);
        assert_eq!(pty_manager.lock().unwrap().backend_kind(session_id), None);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());
        assert!(crate::shutdown_persistence_allowed(true, report.terminal));

        let second = backend.stop_all_started_containers_blocking(Duration::from_millis(100));
        assert!(second.terminal, "retained={:?}", second.retained);
        assert_eq!(runtime.stopped(), vec![session_id]);
    }

    #[test]
    fn blocked_global_sweep_route_remover_returns_at_deadline_with_retained_owner() {
        let runtime = Arc::new(RecordingRuntime::default());
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        let session_id = Uuid::new_v4();
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });

        let route_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&route_calls);
        let (route_entered, route_entered_rx) = std::sync::mpsc::channel();
        let (route_release, route_release_rx) = std::sync::mpsc::channel();
        let route_release_rx = Arc::new(Mutex::new(route_release_rx));
        let release = Arc::clone(&route_release_rx);
        backend.set_route_remover(Arc::new(move |removed_session, _| {
            assert_eq!(removed_session, session_id);
            calls.fetch_add(1, Ordering::SeqCst);
            route_entered
                .send(())
                .expect("publish blocked route-remover entry");
            release
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .recv()
                .expect("release blocked route remover");
            Ok(())
        }));

        let budget = Duration::from_millis(150);
        let (report_sender, report_receiver) = std::sync::mpsc::channel();
        let sweep_backend = Arc::clone(&backend);
        let sweep = std::thread::spawn(move || {
            let started_at = Instant::now();
            let report = sweep_backend.stop_all_started_containers_blocking(budget);
            report_sender
                .send((started_at.elapsed(), report))
                .expect("publish blocked route-remover report");
        });

        route_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("global sweep enters the deterministic blocked route remover");
        let (elapsed, report) = report_receiver
            .recv_timeout(budget + Duration::from_millis(500))
            .expect("global sweep returns while route remover remains blocked");
        sweep.join().expect("join bounded global sweep caller");

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(
            elapsed <= budget + Duration::from_millis(400),
            "blocked route remover exceeded the absolute deadline: {elapsed:?}"
        );
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            backend.retained_runtime_cleanup_sessions_for_test(),
            vec![session_id]
        );
        assert!(runtime.stopped().is_empty());
        assert_eq!(route_calls.load(Ordering::SeqCst), 1);
        assert!(
            report.retained.iter().any(|context| {
                context.contains("owner=containerCleanup")
                    && context.contains(&format!("session={session_id}"))
                    && context.contains("runtimeHandle=true")
                    && context.contains("state=inFlight")
                    && context.contains("inFlight=true")
            }),
            "retained={:?}",
            report.retained
        );
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

        route_release.send(()).expect("release route remover owner");
        let cleanup_deadline = Instant::now() + Duration::from_secs(1);
        while runtime.stopped() != vec![session_id] {
            assert!(
                Instant::now() < cleanup_deadline,
                "retained cleanup did not finish after route-remover release"
            );
            std::thread::yield_now();
        }
        while !backend
            .retained_runtime_cleanup_sessions_for_test()
            .is_empty()
        {
            assert!(
                Instant::now() < cleanup_deadline,
                "retained ownership did not become terminal after release"
            );
            std::thread::yield_now();
        }
        assert_eq!(route_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn poisoned_global_sweep_route_removal_is_structured_residue() {
        let runtime = Arc::new(RecordingRuntime::default());
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        );
        let session_id = Uuid::new_v4();
        let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
            session_id,
            container_id: format!("container-{session_id}"),
        });
        backend.set_route_remover(Arc::new(|_, _| {
            Err(RouteRemovalError::LockPoisoned("ptyManager"))
        }));

        let report = backend.stop_all_started_containers_blocking(Duration::from_millis(50));

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(!backend.contains_transport_state_for_test(session_id));
        assert_eq!(
            backend.retained_runtime_cleanup_sessions_for_test(),
            vec![session_id]
        );
        assert!(runtime.stopped().is_empty());
        assert!(
            report.retained.iter().any(|context| {
                context.contains("owner=containerCleanup")
                    && context.contains(&format!("session={session_id}"))
                    && context.contains("runtimeHandle=true")
                    && context.contains("route removal lock poisoned owner=ptyManager")
            }),
            "retained={:?}",
            report.retained
        );
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));
    }

    #[test]
    fn docker_command_owner_stays_visible_during_real_global_sweep_retry() {
        let runtime = Arc::new(crate::pty::docker_runtime::DockerRuntime::new());
        let readers = runtime.retain_blocked_command_readers_for_test(
            "global-sweep-reader",
            "docker-reader-fixture",
            "reader cleanup failed OPENAI_API_KEY=sk-proj-global-sweep-secret\nnested failure",
        );
        assert_eq!(runtime.active_reader_count(), 2);
        let (retry_entered, retry_release) = runtime.install_command_retry_gate_for_test();
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime.clone(),
            None,
        ));
        let budget = Duration::from_millis(150);
        let (report_sender, report_receiver) = std::sync::mpsc::channel();
        let sweep_backend = Arc::clone(&backend);
        let sweep = std::thread::spawn(move || {
            let started_at = Instant::now();
            let report = sweep_backend.stop_all_started_containers_blocking(budget);
            report_sender
                .send((started_at.elapsed(), report))
                .expect("publish blocked Docker-command global-sweep report");
        });

        retry_entered
            .recv_timeout(Duration::from_secs(1))
            .expect("production runtime retry publishes its in-flight transition");
        let (elapsed, report) = report_receiver
            .recv_timeout(budget + Duration::from_millis(500))
            .expect("global sweep returns while the Docker command retry remains active");
        sweep
            .join()
            .expect("join bounded Docker-command sweep caller");

        assert!(!report.terminal, "retained={:?}", report.retained);
        assert!(
            elapsed <= budget + Duration::from_millis(400),
            "in-flight Docker command exceeded the global-sweep deadline: {elapsed:?}"
        );
        let expected = "owner=dockerCommand session=none reason=global-sweep-reader program=docker-reader-fixture runtimeHandle=none state=inFlight inFlight=true lastError=reader cleanup failed OPENAI_API_KEY=[REDACTED]\\nnested failure";
        assert_eq!(
            report
                .retained
                .iter()
                .filter(|context| context.as_str() == expected)
                .count(),
            1,
            "retained={:?}",
            report.retained
        );
        assert!(report.retained.iter().all(|context| {
            !context.contains("sk-proj-global-sweep-secret")
                && !context.contains("00000000-0000-0000-0000-000000000000")
        }));
        let live_contexts = runtime.retained_cleanup_contexts();
        assert_eq!(live_contexts.len(), 1);
        assert_eq!(live_contexts[0].owner, "dockerCommand");
        assert_eq!(live_contexts[0].session_id, None);
        assert_eq!(live_contexts[0].reason, "global-sweep-reader");
        assert_eq!(
            live_contexts[0].program.as_deref(),
            Some("docker-reader-fixture")
        );
        assert_eq!(live_contexts[0].state, "inFlight");
        assert!(live_contexts[0].in_flight);
        assert_eq!(runtime.active_reader_count(), 2);
        assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

        let mut selection_retained = (0..300)
            .map(|index| {
                format!(
                    "owner=selectionFixture session=none reason=fixture-{index:03} program=none runtimeHandle=none state=retained inFlight=false"
                )
            })
            .collect::<Vec<_>>();
        selection_retained.push(expected.to_string());
        let mut uncapped = selection_retained
            .iter()
            .map(|context| {
                crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                    "selection",
                    context,
                )
            })
            .chain(report.retained.iter().map(|context| {
                crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                    "containerShutdown",
                    context,
                )
            }))
            .collect::<Vec<_>>();
        uncapped.sort();
        uncapped.dedup();
        let combined = crate::combined_shutdown_retained_diagnostics(
            selection_retained,
            report.retained.clone(),
        );
        let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
        assert_eq!(combined.len(), RETAINED_OWNER_REPORT_CAPACITY);
        assert_eq!(
            combined
                .iter()
                .filter(|context| context.as_str() == expected)
                .count(),
            1,
            "combined={combined:?}"
        );
        assert_eq!(
            combined.last().expect("global omitted-count sentinel"),
            &format!(
                "owner=diagnosticSummary session=none reason=retained-owner-report-truncated program=none runtimeHandle=none state=retained inFlight=false omittedCount={expected_omitted}"
            )
        );

        retry_release
            .send(())
            .expect("release the deterministic in-flight retry gate");
        let transition_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let contexts = runtime.retained_cleanup_contexts();
            if contexts
                .first()
                .is_some_and(|context| context.state == "retained" && !context.in_flight)
            {
                break;
            }
            assert!(
                Instant::now() < transition_deadline,
                "Docker command owner did not return to retained state after retry release: {contexts:?}"
            );
            std::thread::yield_now();
        }
        assert_eq!(runtime.active_reader_count(), 2);
        readers.release();
        let retry_control = ContainerRuntimeControl::default();
        retry_control.request_shutdown(Instant::now() + Duration::from_secs(1));
        runtime.retry_retained_cleanups(&retry_control);
        assert!(runtime.retained_cleanup_contexts().is_empty());
        assert_eq!(runtime.active_reader_count(), 0);

        let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(terminal.terminal, "retained={:?}", terminal.retained);
        assert!(terminal.retained.is_empty());
    }

    #[test]
    fn high_cardinality_retained_scheduling_obeys_the_advertised_deadline() {
        const INSTALLED_ENTRIES: usize = 180;
        const RUNTIME_ENTRIES: usize = 180;

        let runtime_contexts = (0..RUNTIME_ENTRIES)
            .map(|index| {
                if index % 2 == 0 {
                    RetainedContainerOwnerContext {
                        owner: "ambiguousStartCleanup",
                        session_id: Some(Uuid::from_u128(10_000 + index as u128)),
                        reason: "ambiguous-start".to_string(),
                        program: None,
                        runtime_handle: Some(true),
                        state: "retained",
                        in_flight: false,
                        last_error: None,
                    }
                } else {
                    RetainedContainerOwnerContext {
                        owner: "dockerCommand",
                        session_id: None,
                        reason: "docker-command".to_string(),
                        program: Some(format!("docker-reader-{index}")),
                        runtime_handle: None,
                        state: "retained",
                        in_flight: false,
                        last_error: None,
                    }
                }
            })
            .collect();
        let runtime = Arc::new(RetainedContextRuntime {
            contexts: runtime_contexts,
        });
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            None,
            None,
            runtime,
            None,
        );
        for index in 0..INSTALLED_ENTRIES {
            let session_id = Uuid::from_u128(20_000 + index as u128);
            let _receiver = backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id,
                container_id: format!("container-{session_id}"),
            });
        }

        let budget = Duration::ZERO;
        let started_at = Instant::now();
        let report = backend.stop_all_started_containers_blocking(budget);
        let elapsed = started_at.elapsed();

        assert!(!report.terminal);
        assert!(
            elapsed <= Duration::from_millis(250),
            "high-cardinality scheduling exceeded its advertised deadline: {elapsed:?}"
        );
        assert_eq!(
            report
                .retained
                .iter()
                .filter(|context| context.contains("owner=installedContainer "))
                .count(),
            INSTALLED_ENTRIES
        );
        assert_eq!(
            report
                .retained
                .iter()
                .filter(|context| {
                    context.contains("owner=ambiguousStartCleanup ")
                        || context.contains("owner=dockerCommand ")
                })
                .count(),
            RUNTIME_ENTRIES
        );

        let selection_retained = vec![
            "reason=selection-coordinator state=retained".to_string(),
            "owner=pendingCreate session=none reason=create state=retained".to_string(),
        ];
        let mut uncapped = selection_retained
            .iter()
            .map(|context| {
                crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                    "selection",
                    context,
                )
            })
            .chain(report.retained.iter().map(|context| {
                crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                    "containerShutdown",
                    context,
                )
            }))
            .collect::<Vec<_>>();
        uncapped.sort();
        uncapped.dedup();
        assert!(uncapped
            .iter()
            .any(|context| context.starts_with("owner=selection ")));
        assert!(uncapped
            .iter()
            .any(|context| context.starts_with("owner=installedContainer ")));
        assert!(uncapped
            .iter()
            .any(|context| context.starts_with("owner=dockerCommand ")));
        assert!(uncapped
            .iter()
            .any(|context| context.starts_with("owner=ambiguousStartCleanup ")));

        let retained =
            crate::combined_shutdown_retained_diagnostics(selection_retained, report.retained);
        let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
        assert_eq!(retained.len(), RETAINED_OWNER_REPORT_CAPACITY);
        assert_eq!(
            retained.last().expect("omitted-count sentinel"),
            &format!(
                "owner=diagnosticSummary session=none reason=retained-owner-report-truncated program=none runtimeHandle=none state=retained inFlight=false omittedCount={expected_omitted}"
            )
        );
    }

    #[tokio::test]
    async fn shutdown_barrier_rejects_start_and_stop_registration_after_empty_drain() {
        let runtime = Arc::new(RecordingRuntime::default());
        let token_dir = tempfile::TempDir::new().unwrap();
        let token_manager =
            ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
        let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning::default());
        backend.runtime = Some(runtime.clone());
        backend.token_manager = Some(token_manager);
        backend.runtime_settings_override = Some(api_enabled_settings());
        backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
        assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

        let rejected_id = Uuid::new_v4();
        let mut spec = test_spec(
            rejected_id,
            "C:/repo/.ac/wg-1/__agent_dev",
            PtyOutputTarget::noop(),
        );
        spec.container_image = Some("agentscommander/test:latest".to_string());
        let error = backend
            .spawn(spec)
            .await
            .expect_err("sealed shutdown barrier rejects a new runtime start");
        assert!(error
            .to_string()
            .contains("container runtime start rejected during shutdown"));
        assert_eq!(runtime.start_count(), 0);
        assert!(!backend.contains_transport_state_for_test(rejected_id));
        assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

        let stopped_id = Uuid::new_v4();
        let _transport_receiver =
            backend.insert_active_runtime_handle_for_test(ContainerRuntimeHandle {
                session_id: stopped_id,
                container_id: format!("container-{stopped_id}"),
            });
        backend
            .kill(stopped_id)
            .expect("sealed registry transfers late stop ownership");
        assert!(runtime.stopped().is_empty());
        assert_eq!(
            backend.retained_cleanup_sessions_for_test(),
            vec![stopped_id]
        );
        assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));

        let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
        assert!(report.terminal, "retained={:?}", report.retained);
        assert_eq!(runtime.stopped(), vec![stopped_id]);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());
        assert_eq!(backend.shutdown_work_state_for_test(), (true, 0, 0));
    }

    #[test]
    fn shutdown_barrier_waits_for_presealed_producer_and_its_late_cleanup() {
        let registry = Arc::new(ContainerShutdownWorkRegistry::default());
        let producer = registry
            .register_producer()
            .expect("register producer before shutdown sealing");
        let (drain_done, drain_done_rx) = std::sync::mpsc::channel();
        let drain_registry = Arc::clone(&registry);
        let drain = std::thread::spawn(move || {
            drain_registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
            drain_done
                .send(())
                .expect("publish shutdown drain completion");
        });
        let seal_deadline = Instant::now() + Duration::from_secs(1);
        while !registry.snapshot().0 {
            assert!(
                Instant::now() < seal_deadline,
                "shutdown registry did not seal"
            );
            std::thread::yield_now();
        }
        assert_eq!(registry.snapshot(), (true, 1, 0));
        assert!(
            drain_done_rx
                .recv_timeout(Duration::from_millis(25))
                .is_err(),
            "an admitted producer must prevent an empty shutdown drain"
        );

        let (cleanup_started, cleanup_started_rx) = std::sync::mpsc::channel();
        let (cleanup_release, cleanup_release_rx) = std::sync::mpsc::channel();
        producer.spawn_owned(None, "barrier-test", move |_| {
            cleanup_started
                .send(())
                .expect("publish late cleanup start");
            cleanup_release_rx
                .recv()
                .expect("wait for late cleanup release");
        });
        drop(producer);
        cleanup_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("presealed producer registers its late cleanup");
        assert!(
            drain_done_rx
                .recv_timeout(Duration::from_millis(25))
                .is_err(),
            "shutdown drain must join cleanup registered after sealing"
        );
        cleanup_release.send(()).expect("release late cleanup");
        drain_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown drain completes after late cleanup");
        drain.join().expect("join shutdown drain thread");
        assert_eq!(registry.snapshot(), (true, 0, 0));
        assert!(registry.register_producer().is_none());
    }

    #[test]
    fn bounded_shutdown_executor_limits_multiple_slow_cleanups() {
        const WORK_ITEMS: usize = 12;

        let registry = Arc::new(ContainerShutdownWorkRegistry::default());
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        for _ in 0..WORK_ITEMS {
            let producer = registry
                .register_producer()
                .expect("register bounded-executor producer");
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let completed = Arc::clone(&completed);
            producer.spawn_owned(None, "bounded-cleanup-test", move |control| {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current, Ordering::SeqCst);
                let _ = control.wait_for_shutdown();
                active.fetch_sub(1, Ordering::SeqCst);
                completed.fetch_add(1, Ordering::SeqCst);
            });
        }

        let start_deadline = Instant::now() + Duration::from_secs(1);
        while active.load(Ordering::SeqCst) < CONTAINER_SHUTDOWN_WORKER_CAPACITY {
            assert!(
                Instant::now() < start_deadline,
                "bounded shutdown workers did not start"
            );
            std::thread::yield_now();
        }
        assert_eq!(registry.worker_count(), CONTAINER_SHUTDOWN_WORKER_CAPACITY);
        assert_eq!(registry.snapshot(), (false, 0, WORK_ITEMS));
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            CONTAINER_SHUTDOWN_WORKER_CAPACITY
        );

        registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
        assert_eq!(completed.load(Ordering::SeqCst), WORK_ITEMS);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(max_active.load(Ordering::SeqCst) <= CONTAINER_SHUTDOWN_WORKER_CAPACITY);
        assert_eq!(registry.snapshot(), (true, 0, 0));
        assert_eq!(registry.worker_count(), 0);
    }

    #[test]
    fn worker_spawn_failure_serializes_multiple_slow_cleanups() {
        const WORK_ITEMS: usize = 4;

        let registry = Arc::new(ContainerShutdownWorkRegistry::default());
        registry.inject_worker_spawn_failure();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let mut callers = Vec::new();
        for index in 0..WORK_ITEMS {
            let producer = registry
                .register_producer()
                .expect("register synchronous-fallback producer");
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let completed = Arc::clone(&completed);
            callers.push(
                std::thread::Builder::new()
                    .name(format!("shutdown-fallback-caller-{index}"))
                    .spawn(move || {
                        producer.spawn_owned(None, "fallback-bound-test", move |control| {
                            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(current, Ordering::SeqCst);
                            let _ = control.wait_for_shutdown();
                            active.fetch_sub(1, Ordering::SeqCst);
                            completed.fetch_add(1, Ordering::SeqCst);
                        });
                    })
                    .expect("spawn synchronous-fallback caller"),
            );
        }

        let start_deadline = Instant::now() + Duration::from_secs(1);
        while max_active.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < start_deadline,
                "synchronous fallback did not start"
            );
            std::thread::yield_now();
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(registry.worker_count(), 0);
        assert_eq!(registry.snapshot(), (false, WORK_ITEMS, 1));

        registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
        for caller in callers {
            caller.join().expect("join synchronous-fallback caller");
        }
        assert_eq!(completed.load(Ordering::SeqCst), WORK_ITEMS);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(registry.snapshot(), (true, 0, 0));
    }

    #[test]
    fn blocked_producer_fallback_is_reported_at_the_absolute_deadline() {
        let registry = Arc::new(ContainerShutdownWorkRegistry::default());
        registry.inject_worker_spawn_failure();
        let producer = registry
            .register_producer()
            .expect("register blocked-fallback producer");
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            producer.spawn_owned(None, "blocked-fallback-test", move |_| {
                started.send(()).expect("publish blocked fallback start");
                release_rx.recv().expect("release blocked fallback");
            });
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked fallback starts");

        let budget = Duration::from_millis(100);
        let started_at = Instant::now();
        let report = registry.seal_and_drain_until(started_at + budget);
        assert!(!report.terminal);
        assert!(
            started_at.elapsed() <= budget + Duration::from_millis(250),
            "blocked fallback drain exceeded its absolute deadline: {:?}",
            started_at.elapsed()
        );
        assert_eq!(
            report
                .retained
                .iter()
                .filter(|context| context.contains("reason=blocked-fallback-test"))
                .count(),
            1,
            "retained={:?}",
            report.retained
        );

        release.send(()).expect("release blocked fallback owner");
        caller.join().expect("join blocked fallback caller");
        let terminal = registry.seal_and_drain_until(Instant::now() + Duration::from_secs(1));
        assert!(terminal.terminal, "retained={:?}", terminal.retained);
        assert_eq!(registry.snapshot(), (true, 0, 0));
    }

    #[tokio::test]
    async fn canceled_handshake_stops_installed_runtime_handle_and_removes_pending_state() {
        let id = Uuid::new_v4();
        let root_dir = tempfile::TempDir::new().unwrap();
        let root = root_dir.path().to_string_lossy().into_owned();
        let runtime = Arc::new(RecordingRuntime::default());
        let token_dir = tempfile::TempDir::new().unwrap();
        let token_manager =
            ContainerApiTokenManager::new_for_path(token_dir.path().join("api-clients.json"));
        let (mut backend, _manager) = backend_with_tuning(ContainerTransportTuning {
            handshake_timeout: Duration::from_secs(30),
            ..ContainerTransportTuning::default()
        });
        backend.runtime = Some(runtime.clone());
        backend.token_manager = Some(token_manager);
        backend.runtime_settings_override = Some(api_enabled_settings());
        let backend = Arc::new(backend);
        let task_backend = Arc::clone(&backend);
        let mut spec = test_spec(id, &root, PtyOutputTarget::noop());
        spec.container_image = Some("agentscommander/test:latest".to_string());
        let task = tokio::spawn(async move { task_backend.spawn(spec).await });

        let mut installed = false;
        for _ in 0..100 {
            installed = matches!(
                backend.sessions.lock().unwrap().get(&id),
                Some(ContainerSessionState::Pending(pending))
                    if pending.runtime_handle.is_some()
            );
            if installed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            installed,
            "runtime handle was not installed before handshake wait"
        );
        task.abort();
        let _ = task.await;

        for _ in 0..100 {
            if runtime.stopped().contains(&id)
                && !backend.sessions.lock().unwrap().contains_key(&id)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("canceled handshake leaked its runtime handle or pending state");
    }

    // #930 plan section 9.a - a teardown funnel (kill -> remove_session_state ->
    // cleanup_removed_resources_async) must delete the copied host credential so
    // no live refresh token is left in the workspace tree.
    #[test]
    fn teardown_deletes_copied_credential() {
        let id = Uuid::new_v4();
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join(".claude").join(".credentials.json");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"refresh-token-bytes").unwrap();
        assert!(dest.is_file(), "precondition: credential present");

        let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
        // No runtime/token_manager: cleanup runs remove_copied first, then the
        // runtime-stop and token-revoke branches are skipped (both None).
        let (tx, _rx) = mpsc::channel(8);
        backend.sessions.lock().unwrap().insert(
            id,
            ContainerSessionState::Active(ActiveSession {
                output_target: PtyOutputTarget::noop(),
                sender: tx,
                rows: 30,
                cols: 120,
                runtime_handle: None,
                api_client_id: None,
                credential_binding: None,
                logical_resource_slot: None,
                container_credential_path: Some(dest.clone()),
            }),
        );

        backend.kill(id).expect("kill");
        let report =
            backend.seal_and_drain_shutdown_work_blocking(Instant::now() + Duration::from_secs(1));
        assert!(report.terminal, "retained={:?}", report.retained);
        assert!(backend.retained_cleanup_sessions_for_test().is_empty());

        assert!(!dest.exists(), "teardown must delete the copied credential");
    }

    // #930 F1 - the teardown-during-spawn race: if the session is gone from the
    // map at the post-copy_in recheck, the recheck deletes the orphaned file;
    // if it is still registered, the file survives for the container. Drives the
    // real `remove_credential_if_orphaned` (the extracted recheck).
    #[test]
    fn f1_recheck_removes_credential_when_session_gone() {
        let id = Uuid::new_v4();
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join(".claude").join(".credentials.json");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"refresh-token-bytes").unwrap();
        let plan = crate::pty::container_credentials::ContainerCredentialPlan {
            source: dir.path().join("unused-source"),
            dest: dest.clone(),
            first_run: None,
        };

        let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
        let (tx, _rx) = mpsc::channel(8);
        backend.sessions.lock().unwrap().insert(
            id,
            ContainerSessionState::Active(ActiveSession {
                output_target: PtyOutputTarget::noop(),
                sender: tx,
                rows: 30,
                cols: 120,
                runtime_handle: None,
                api_client_id: None,
                credential_binding: None,
                logical_resource_slot: None,
                container_credential_path: Some(dest.clone()),
            }),
        );

        // Session still registered (normal spawn): recheck is a no-op, file survives.
        backend.remove_credential_if_orphaned(id, &plan);
        assert!(
            dest.is_file(),
            "file must survive while the session is registered"
        );

        // Session removed before the recheck (teardown-during-spawn): recheck deletes.
        backend.sessions.lock().unwrap().remove(&id);
        backend.remove_credential_if_orphaned(id, &plan);
        assert!(
            !dest.exists(),
            "recheck must delete the orphaned credential when the session is gone"
        );
    }

    #[test]
    fn sanitized_child_env_removes_reserved_credentials_and_removed_keys() {
        let env = vec![
            ("CODEX_HOME".to_string(), "/workspace/.codex".to_string()),
            (
                "AGENTSCOMMANDER_TOKEN".to_string(),
                "session-token".to_string(),
            ),
            (
                "AGENTSCOMMANDER_BINARY_PATH".to_string(),
                "C:/host/ac.exe".to_string(),
            ),
            ("DROP_ME".to_string(), "x".to_string()),
        ];
        let got = sanitized_child_env(env, vec!["drop_me".to_string()]);

        assert_eq!(
            got,
            vec![("CODEX_HOME".to_string(), "/workspace/.codex".to_string())]
        );
    }

    fn child_env_map() -> ContainerPathMap {
        ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
    }

    #[test]
    fn container_child_env_translates_mappable_host_path_keys() {
        let got = container_child_env(
            vec![
                (
                    CLAUDE_CONFIG_DIR_KEY.to_string(),
                    r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude".to_string(),
                ),
                ("MY_VAR".to_string(), r"C:\Users\maria\.outside".to_string()),
            ],
            Vec::new(),
            &child_env_map(),
        );

        assert_eq!(
            got.child_env,
            vec![
                (
                    CLAUDE_CONFIG_DIR_KEY.to_string(),
                    "/workspace/.claude".to_string()
                ),
                ("MY_VAR".to_string(), r"C:\Users\maria\.outside".to_string())
            ]
        );
        assert!(got.env_unset.is_empty());
        assert!(got.warnings.is_empty());
    }

    #[test]
    fn container_child_env_unsets_unmappable_host_path_keys() {
        let got = container_child_env(
            vec![
                (
                    CLAUDE_CONFIG_DIR_KEY.to_string(),
                    r"C:\Users\maria\.claude".to_string(),
                ),
                (
                    crate::pty::container_paths::CODEX_HOME_KEY.to_string(),
                    "/workspace/.codex".to_string(),
                ),
            ],
            Vec::new(),
            &child_env_map(),
        );

        assert!(got.child_env.is_empty());
        assert_eq!(
            got.env_unset,
            vec![
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                crate::pty::container_paths::CODEX_HOME_KEY.to_string()
            ]
        );
        assert_eq!(
            got.warnings
                .iter()
                .map(|warning| (warning.key.as_str(), warning.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    CLAUDE_CONFIG_DIR_KEY,
                    crate::pty::container_paths::WARNING_KIND_OUTSIDE_MOUNT
                ),
                (
                    crate::pty::container_paths::CODEX_HOME_KEY,
                    crate::pty::container_paths::WARNING_KIND_CONTAINER_PATH_IN_HOST_FIELD
                )
            ]
        );
    }

    #[test]
    fn container_child_env_removal_suppresses_unset_and_warning() {
        let got = container_child_env(
            vec![(
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                r"C:\Users\maria\.claude".to_string(),
            )],
            vec![CLAUDE_CONFIG_DIR_KEY.to_string()],
            &child_env_map(),
        );

        assert!(got.child_env.is_empty());
        assert!(got.env_unset.is_empty());
        assert!(got.warnings.is_empty());
    }

    fn api_enabled_settings() -> crate::config::settings::AppSettings {
        crate::config::settings::AppSettings {
            api_server_enabled: true,
            api_server_bind: "0.0.0.0".to_string(),
            api_server_port: 8765,
            ..crate::config::settings::AppSettings::default()
        }
    }

    fn token() -> ContainerApiToken {
        ContainerApiToken {
            client_id: "client".to_string(),
            credential_generation: Uuid::new_v4().to_string(),
            bound_session_id: Uuid::new_v4().to_string(),
            secret: "secret".to_string(),
            token_hash: crate::api::auth::hash_token("secret"),
        }
    }

    #[test]
    fn build_start_request_honors_container_image_override() {
        let id = Uuid::new_v4();
        let request = build_start_request_with_settings(
            id,
            "claude",
            Vec::new(),
            "C:/repo/.ac/wg-1/__agent_dev",
            30,
            120,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(" agentscommander/ac-claude:latest ".to_string()),
            None,
            "ticket".to_string(),
            &token(),
            &api_enabled_settings(),
            Vec::new(),
        )
        .unwrap();

        assert_eq!(request.image, "agentscommander/ac-claude:latest");
    }

    #[test]
    fn handshake_timeout_error_includes_container_diagnostics() {
        let diagnostics = ContainerDiagnostics {
            container_id: "container-123".to_string(),
            state: Some(crate::pty::container_runtime::ContainerStateSnapshot {
                status: Some("exited".to_string()),
                running: Some(false),
                exit_code: Some(127),
                error: None,
            }),
            inspect_error: None,
            log_tail: Some("session-bridge error: command not found: claude".to_string()),
            logs_error: None,
        };

        let err = ContainerTransportBackend::handshake_timeout_error(
            Duration::from_secs(5),
            Some(&diagnostics),
        )
        .to_string();

        assert!(
            err.contains("container bridge did not attach within 5s"),
            "{err}"
        );
        assert!(err.contains("container id container-123"), "{err}");
        assert!(err.contains("exitCode=127"), "{err}");
        assert!(err.contains("command not found: claude"), "{err}");
    }

    #[test]
    fn handshake_timeout_error_leads_when_diagnostics_fail() {
        let diagnostics = ContainerDiagnostics {
            container_id: "container-123".to_string(),
            state: None,
            inspect_error: Some("inspect failed".to_string()),
            log_tail: None,
            logs_error: Some("logs failed".to_string()),
        };

        let err = ContainerTransportBackend::handshake_timeout_error(
            Duration::from_secs(5),
            Some(&diagnostics),
        )
        .to_string();

        assert!(
            err.starts_with("PTY error: container bridge did not attach within 5s"),
            "{err}"
        );
        assert!(err.contains("container id container-123"), "{err}");
        assert!(err.contains("logs unavailable: logs failed"), "{err}");
    }

    #[tokio::test]
    async fn valid_ticket_plus_hello_registers_container_session() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;

        let _rx = attach(&backend, id, root, &ticket);

        assert!(backend.has_session(id));
    }

    #[tokio::test]
    async fn ticket_rejects_missing_expired_consumed_wrong_session_and_wrong_root() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;

        assert_eq!(
            backend.consume_ticket(id, root, "wrong").unwrap_err(),
            TransportTicketError::Invalid
        );
        assert_eq!(
            backend
                .consume_ticket(Uuid::new_v4(), root, &ticket)
                .unwrap_err(),
            TransportTicketError::Invalid
        );
        assert_eq!(
            backend
                .consume_ticket(id, "C:/other/.ac/wg-1/__agent_dev", &ticket)
                .unwrap_err(),
            TransportTicketError::Invalid
        );

        backend.consume_ticket(id, root, &ticket).expect("consume");
        assert_eq!(
            backend.consume_ticket(id, root, &ticket).unwrap_err(),
            TransportTicketError::Invalid
        );

        let expired_id = Uuid::new_v4();
        let tuning = ContainerTransportTuning {
            ticket_ttl: Duration::from_millis(1),
            ..ContainerTransportTuning::default()
        };
        let (expired_backend, _mgr) = backend_with_tuning(tuning);
        expired_backend
            .spawn(test_spec(expired_id, root, PtyOutputTarget::noop()))
            .await
            .expect("spawn expired");
        let expired_ticket = expired_backend
            .last_issued_ticket_for_test(expired_id)
            .expect("expired ticket");
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(
            expired_backend
                .consume_ticket(expired_id, root, &expired_ticket)
                .unwrap_err(),
            TransportTicketError::Invalid
        );
    }

    #[tokio::test]
    async fn output_before_hello_is_rejected() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;
        backend
            .consume_ticket(id, root, &ticket)
            .expect("consume ticket");

        let err = backend
            .handle_bridge_output(id, b"early".to_vec())
            .expect_err("output before hello");

        assert!(matches!(err, AppError::SessionNotFound(_)));
    }

    #[tokio::test]
    async fn binary_output_reaches_fanout_payload_shape() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let captured = Arc::new(Mutex::new(Vec::new()));
        let target = PtyOutputTarget::from_test_sink(captured.clone());
        let (backend, _mgr) = backend_with_tuning(ContainerTransportTuning::default());
        backend
            .spawn(test_spec(id, root, target))
            .await
            .expect("spawn");
        let ticket = backend.last_issued_ticket_for_test(id).unwrap();
        let _rx = attach(&backend, id, root, &ticket);

        backend
            .handle_bridge_output(id, b"hello".to_vec())
            .expect("output");

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, id.to_string());
        assert_eq!(got[0].1, b"hello".to_vec());
        assert_eq!(got[0].2, Some(1));
    }

    #[tokio::test]
    async fn facade_write_sends_binary_input_to_bridge() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;
        let mut rx = attach(&backend, id, root, &ticket);

        backend
            .write(
                &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
                id,
                b"abc",
            )
            .expect("write");

        assert_eq!(
            rx.recv().await.expect("bridge frame"),
            HostToBridgeFrame::Binary(b"abc".to_vec())
        );
    }

    #[tokio::test]
    async fn resize_sends_frame_and_updates_size() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;
        let mut rx = attach(&backend, id, root, &ticket);

        backend.resize(id, 100, 40).expect("resize");

        assert_eq!(
            rx.recv().await.expect("resize frame"),
            HostToBridgeFrame::Text(HostToBridgeTextFrame::Resize {
                version: TRANSPORT_PROTOCOL_VERSION,
                cols: 100,
                rows: 40,
            })
        );
        assert_eq!(backend.get_pty_size(id), Some((40, 100)));
    }

    #[tokio::test]
    async fn duplicate_attach_for_same_session_is_rejected() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let (backend, ticket) = pending_backend(id, root).await;
        let _rx = attach(&backend, id, root, &ticket);
        let (tx, _rx2) = mpsc::channel(8);

        assert_eq!(
            backend.complete_hello(id, root, tx).unwrap_err(),
            TransportAttachError::Invalid
        );
    }

    #[tokio::test]
    async fn exit_and_disconnect_cleanup_remove_route_and_transport_state() {
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        for code in [7, TRANSPORT_LOST_EXIT_CODE] {
            let (backend, session_mgr) = backend_with_tuning(ContainerTransportTuning::default());
            let removed = Arc::new(AtomicUsize::new(0));
            let removed_for_cb = removed.clone();
            backend.set_route_remover(Arc::new(move |_, _| {
                removed_for_cb.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
            let session = session_mgr
                .read()
                .await
                .create_session(
                    "container".to_string(),
                    Vec::new(),
                    root.to_string(),
                    Some("agent".to_string()),
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::ContainerTransport,
                )
                .await
                .expect("session");
            backend
                .spawn(test_spec(session.id, root, PtyOutputTarget::noop()))
                .await
                .expect("spawn");
            let ticket = backend.last_issued_ticket_for_test(session.id).unwrap();
            let _rx = attach(&backend, session.id, root, &ticket);

            if code == 7 {
                backend.handle_bridge_exit(session.id, code).await;
            } else {
                backend.handle_bridge_disconnect(session.id).await;
            }

            assert_eq!(removed.load(Ordering::SeqCst), 1);
            assert!(!backend.has_session(session.id));
            assert!(!backend.has_session(session.id));
        }
    }

    /// #1171, 9.1.4 (container half) - an id this backend has no parser for is `Gone`, not
    /// `Missing`.
    ///
    /// This backend tears down on a natural exit and `close_transport` drops the parser before
    /// anyone could read it, so parser-absent IS its liveness oracle - the same reading its
    /// `get_screen_rows` already gives (`:3171-3179`). The local backend answers `Missing` to
    /// the identical question, which is the whole reason `ScreenRowsSince` has four variants.
    #[test]
    fn screen_rows_since_reports_gone_for_an_unknown_id() {
        let backend = ContainerTransportBackend::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
            None,
        );

        assert!(matches!(
            backend.screen_rows_since(Uuid::new_v4(), None),
            ScreenRowsSince::Gone
        ));
    }

    #[tokio::test]
    async fn synchronous_queue_full_cleanup_defers_outer_route_removal_past_pty_lock() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let output_senders = Arc::new(Mutex::new(HashMap::new()));
        let idle = IdleDetector::new(|_| {}, |_| {});
        let dummy_local = Arc::new(ContainerTransportBackend::new(
            output_senders,
            idle,
            None,
            None,
        ));
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            dummy_local,
        )));
        let backend = pty.lock().unwrap().container_backend();
        backend
            .spawn(test_spec(id, root, PtyOutputTarget::noop()))
            .await
            .expect("spawn pending transport");
        let ticket = backend.last_issued_ticket_for_test(id).unwrap();
        let _receiver = attach(&backend, id, root, &ticket);
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::ContainerTransport);

        let weak_pty = Arc::downgrade(&pty);
        let (removed_tx, removed_rx) = std::sync::mpsc::channel();
        backend.set_route_remover(Arc::new(move |session_id, _| {
            if let Some(pty) = weak_pty.upgrade() {
                pty.lock()
                    .unwrap()
                    .remove_route_if_kind(session_id, SessionBackendKind::ContainerTransport);
            }
            let _ = removed_tx.send(());
            Ok(())
        }));

        let guard = pty.lock().unwrap();
        let authority = crate::pty::manager::BackendWriteAuthority::for_backend_test();
        for _ in 0..8 {
            backend
                .write(&authority, id, b"x")
                .expect("fill outbound queue");
        }
        let started = Instant::now();
        let error = backend
            .write(&authority, id, b"overflow")
            .expect_err("ninth frame must close a full queue");
        assert!(error.to_string().contains("outbound queue full"));
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(removed_rx.try_recv().is_err());
        assert_eq!(
            guard.backend_kind(id),
            Some(SessionBackendKind::ContainerTransport)
        );
        drop(guard);

        removed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deferred outer route removal");
        assert_eq!(pty.lock().unwrap().backend_kind(id), None);
        assert!(!backend.has_session(id));
    }

    #[tokio::test]
    async fn heartbeat_timeout_close_removes_session() {
        let id = Uuid::new_v4();
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        let tuning = ContainerTransportTuning {
            heartbeat_interval: Duration::from_millis(1),
            max_idle: Duration::from_millis(1),
            ..ContainerTransportTuning::default()
        };
        let (backend, _mgr) = backend_with_tuning(tuning);
        backend
            .spawn(test_spec(id, root, PtyOutputTarget::noop()))
            .await
            .expect("spawn");
        let ticket = backend.last_issued_ticket_for_test(id).unwrap();
        let _rx = attach(&backend, id, root, &ticket);

        tokio::time::sleep(Duration::from_millis(3)).await;
        backend.handle_bridge_disconnect(id).await;

        assert!(!backend.has_session(id));
    }

    #[test]
    fn parses_versioned_frames_and_rejects_large_payloads() {
        let text = serde_json::json!({
            "type": "hello",
            "version": TRANSPORT_PROTOCOL_VERSION,
            "sessionId": Uuid::nil(),
            "root": "C:/root"
        })
        .to_string();
        let frame = parse_bridge_text_frame(&text).expect("parse");
        assert_eq!(frame.version(), TRANSPORT_PROTOCOL_VERSION);

        assert!(b"x".repeat(MAX_TRANSPORT_FRAME_BYTES + 1).len() > MAX_TRANSPORT_FRAME_BYTES);
    }

    // #992 - the posture predicate. Never assert on the spawned thread; assert on the
    // decision it makes.
    fn token_manager_at(dir: &tempfile::TempDir) -> ContainerApiTokenManager {
        ContainerApiTokenManager::new_for_path(dir.path().join("api-clients.json"))
    }

    #[test]
    fn sweep_is_load_bearing_is_false_without_container_clients() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = token_manager_at(&dir);
        assert!(!ContainerTransportBackend::sweep_is_load_bearing(Some(
            &manager
        )));
    }

    #[test]
    fn sweep_is_load_bearing_is_true_after_a_container_token_was_minted() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = token_manager_at(&dir);
        manager
            .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
            .unwrap();
        assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
            &manager
        )));
    }

    #[test]
    fn sweep_is_load_bearing_is_true_for_an_expired_and_revoked_client() {
        // The ex-container user's REAL steady state: the token expired after 24h AND a
        // later startup revoked it. `mint_for_session` cannot build it, because it
        // stamps expiry at +24h; the first cut of this test did exactly that and so
        // never expired anything. A predicate such as
        // `prefix && (!client.revoked || !is_expired(client))` survives every other test
        // here and answers false for this state, downgrading the sweep for precisely the
        // users whose orphans it exists to find.
        use crate::api::auth::{self, MintRequest, SCOPE_SEND};
        use chrono::{Duration as ChronoDuration, Utc};

        let dir = tempfile::TempDir::new().unwrap();
        let manager = token_manager_at(&dir);
        let client_id = format!("container-{}", Uuid::new_v4());
        auth::mint(
            manager.path(),
            MintRequest {
                client_id: client_id.clone(),
                secret: "expired-secret".to_string(),
                label: format!("container:{}", Uuid::new_v4()),
                bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
                bound_fqn: "project:wg-1-team/dev".to_string(),
                scopes: vec![SCOPE_SEND.to_string()],
                issued_at: (Utc::now() - ChronoDuration::hours(72)).to_rfc3339(),
                expires_at: Some((Utc::now() - ChronoDuration::hours(48)).to_rfc3339()),
                bound_session_id: None,
                credential_generation: None,
            },
        )
        .unwrap();
        manager.revoke(&client_id);

        // Prove the fixture is the state the test is named for, or it proves nothing.
        let client = auth::list(manager.path())
            .clients
            .into_iter()
            .find(|client| client.client_id == client_id)
            .expect("the client the test minted");
        let expiry =
            chrono::DateTime::parse_from_rfc3339(client.expires_at.as_deref().expect("expires_at"))
                .expect("rfc3339 expiry")
                .with_timezone(&Utc);
        assert!(
            expiry < Utc::now(),
            "this test must actually expire the client"
        );
        assert!(client.revoked, "this test must actually revoke the client");

        assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
            &manager
        )));
    }

    #[test]
    fn sweep_is_load_bearing_fails_load_bearing_without_a_token_manager() {
        // An unknown answer must never downgrade the sweep.
        assert!(ContainerTransportBackend::sweep_is_load_bearing(None));
    }

    #[test]
    fn sweep_is_load_bearing_fails_load_bearing_on_a_malformed_registry() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("api-clients.json");
        std::fs::write(&path, "{").unwrap();
        let manager = ContainerApiTokenManager::new_for_path(path);
        assert!(ContainerTransportBackend::sweep_is_load_bearing(Some(
            &manager
        )));
    }
}
