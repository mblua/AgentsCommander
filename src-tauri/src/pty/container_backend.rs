use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::container_lifecycle::ContainerLifecycleSender;
use crate::pty::container_runtime::{ContainerRuntime, ContainerRuntimeHandle};
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::pty::context_scrape::ScreenRowsRead;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
use crate::pty::output_senders::OutputSenderMap;
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::resource_monitor::ResourceLogicalAgentSlot;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

// Keep this re-export so session_transport and container_backend continue to
// share exactly one normalization rule.
pub(crate) use crate::pty::container_paths::root_key;

mod cleanup;
mod retained_cleanup;
mod shutdown_work;
mod spawn;
mod start_request;
mod sweep;
#[cfg(test)]
mod test_support;
mod transport;

use retained_cleanup::RetainedContainerCleanupRegistry;
use shutdown_work::ContainerShutdownWorkRegistry;
pub(crate) use start_request::container_child_env;

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
        _authority: &crate::pty::input_route::BackendWriteAuthority,
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

#[cfg(test)]
mod tests;
