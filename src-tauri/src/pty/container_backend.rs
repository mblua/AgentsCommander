use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::container_paths::{
    canonical_host_path_env_key, claude_config_dir_unmappable_warning, container_config_dir,
    ContainerEnvClass, ContainerEnvWarning, ContainerPathMap, CLAUDE_CONFIG_DIR_KEY,
};
use crate::pty::container_runtime::{
    api_url_for_container, resolve_container_image, ContainerDiagnostics, ContainerRuntime,
    ContainerRuntimeHandle, ContainerStartRequest, CONTAINER_STOP_TIMEOUT,
    DEFAULT_CONTAINER_WORKDIR,
};
use crate::pty::container_tokens::{ContainerApiToken, ContainerApiTokenManager};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
use crate::resource_monitor::ResourceLogicalAgentSlot;
use crate::session::manager::SessionManager;
use crate::telegram::manager::OutputSenderMap;

pub const TRANSPORT_PROTOCOL_VERSION: u32 = 2;
pub const MAX_TRANSPORT_FRAME_BYTES: usize = 64 * 1024;
const TRANSPORT_LOST_EXIT_CODE: i32 = 1;
const CLEANUP_TASK_TIMEOUT: Duration = Duration::from_secs(10);
const CONTAINER_DIAGNOSTIC_LOG_TAIL_LINES: usize = 80;

// Keep this re-export so session_transport and container_backend continue to
// share exactly one normalization rule.
pub(crate) use crate::pty::container_paths::root_key;

type RouteRemover = Arc<dyn Fn(Uuid) + Send + Sync>;

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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    attach_notify: Option<Arc<Notify>>,
}

struct AttachingSession {
    root_key: String,
    output_target: PtyOutputTarget,
    idle_tuning: crate::session::profile::IdleTuning,
    rows: u16,
    cols: u16,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    attach_notify: Option<Arc<Notify>>,
}

struct ActiveSession {
    output_target: PtyOutputTarget,
    sender: mpsc::Sender<HostToBridgeFrame>,
    rows: u16,
    cols: u16,
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
}

enum ContainerSessionState {
    Pending(PendingSession),
    Attaching(AttachingSession),
    Active(ActiveSession),
}

struct RemovedSessionResources {
    runtime_handle: Option<ContainerRuntimeHandle>,
    api_client_id: Option<String>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
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
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    route_remover: Arc<Mutex<Option<RouteRemover>>>,
    tuning: ContainerTransportTuning,
    runtime: Option<Arc<dyn ContainerRuntime>>,
    token_manager: Option<ContainerApiTokenManager>,
    #[cfg(test)]
    issued_tickets_for_test: Arc<Mutex<HashMap<Uuid, String>>>,
}

impl ContainerTransportBackend {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Self {
        Self::with_tuning(
            output_senders,
            idle_detector,
            ws_broadcaster,
            session_mgr,
            ContainerTransportTuning::default(),
        )
    }

    pub fn with_tuning(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        tuning: ContainerTransportTuning,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            fanout: SessionIoFanout::new(output_senders, idle_detector, ws_broadcaster),
            session_mgr,
            route_remover: Arc::new(Mutex::new(None)),
            tuning,
            runtime: None,
            token_manager: None,
            #[cfg(test)]
            issued_tickets_for_test: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_runtime(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        runtime: Arc<dyn ContainerRuntime>,
        token_manager: Option<ContainerApiTokenManager>,
    ) -> Self {
        let mut backend = Self::with_tuning(
            output_senders,
            idle_detector,
            ws_broadcaster,
            session_mgr,
            ContainerTransportTuning::default(),
        );
        backend.runtime = Some(runtime);
        backend.token_manager = token_manager;
        backend
    }

    pub fn tuning(&self) -> ContainerTransportTuning {
        self.tuning
    }

    pub fn set_route_remover(&self, remover: RouteRemover) {
        *self.route_remover.lock().unwrap() = Some(remover);
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
            logical_resource_slot: pending.logical_resource_slot,
            attach_notify: pending.attach_notify,
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
                    logical_resource_slot: attach.logical_resource_slot,
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
            let mgr = self.session_mgr.read().await;
            let _ = mgr.mark_exited(session_id, code).await;
            crate::config::sessions_persistence::persist_current_state(&mgr).await;
        }
    }

    fn close_transport_from_sync(&self, session_id: Uuid, exit_code: i32) {
        let resources = self.remove_session_state(session_id);
        let Some(resources) = resources else {
            return;
        };

        self.cleanup_removed_resources_async(resources, "transport-sync-close");
        self.remove_route(session_id);
        let session_mgr = self.session_mgr.clone();
        tauri::async_runtime::spawn(async move {
            let mgr = session_mgr.read().await;
            let _ = mgr.mark_exited(session_id, exit_code).await;
            crate::config::sessions_persistence::persist_current_state(&mgr).await;
        });
    }

    fn remove_session_state(&self, session_id: Uuid) -> Option<RemovedSessionResources> {
        let removed = self.sessions.lock().unwrap().remove(&session_id);
        if removed.is_some() {
            self.fanout.remove_session(session_id);
        }
        removed.map(resources_from_state)
    }

    fn remove_route(&self, session_id: Uuid) {
        let remover = self.route_remover.lock().unwrap().clone();
        if let Some(remove) = remover {
            remove(session_id);
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
            logical_resource_slot,
            attach_notify,
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

    fn install_api_client_id(&self, session_id: Uuid, client_id: String) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().unwrap();
        let state = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        match state {
            ContainerSessionState::Pending(pending) => pending.api_client_id = Some(client_id),
            ContainerSessionState::Attaching(attaching) => {
                attaching.api_client_id = Some(client_id)
            }
            ContainerSessionState::Active(active) => active.api_client_id = Some(client_id),
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
            cmd,
            args,
            cwd,
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
        } = spec;

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
        )?;

        let token = match token_manager.mint_for_session(id, &cwd) {
            Ok(token) => token,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded(resources, "mint-failure")
                        .await;
                }
                return Err(err);
            }
        };
        if let Err(err) = self.install_api_client_id(id, token.client_id.clone()) {
            token_manager.revoke(&token.client_id);
            if let Some(resources) = self.remove_session_state(id) {
                self.cleanup_removed_resources_offloaded(resources, "install-client-failure")
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
            ticket,
            &token,
        ) {
            Ok(request) => request,
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded(resources, "build-request-failure")
                        .await;
                }
                return Err(err);
            }
        };
        let start_runtime = runtime.clone();
        let start_result = tokio::task::spawn_blocking(move || start_runtime.start(request)).await;
        let handle = match start_result {
            Ok(Ok(handle)) => handle,
            Ok(Err(err)) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded(resources, "runtime-start-failure")
                        .await;
                }
                return Err(err);
            }
            Err(err) => {
                if let Some(resources) = self.remove_session_state(id) {
                    self.cleanup_removed_resources_offloaded(resources, "runtime-start-panic")
                        .await;
                }
                return Err(AppError::Other(format!(
                    "container runtime start task failed: {err}"
                )));
            }
        };
        if let Err(err) = self.install_runtime_handle(id, handle.clone()) {
            let resources = RemovedSessionResources {
                runtime_handle: Some(handle),
                api_client_id: Some(token.client_id),
                logical_resource_slot: None,
            };
            self.cleanup_removed_resources_offloaded(resources, "install-runtime-failure")
                .await;
            return Err(err);
        }

        if self.has_session(id) {
            return Ok(());
        }
        match tokio::time::timeout(self.tuning.handshake_timeout, attach_notify.notified()).await {
            Ok(_) if self.has_session(id) => Ok(()),
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
                    self.cleanup_removed_resources_offloaded(resources, "handshake-timeout")
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

    fn cleanup_removed_resources_async(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) {
        if let (Some(manager), Some(client_id)) =
            (self.token_manager.clone(), resources.api_client_id.clone())
        {
            manager.revoke(&client_id);
        }
        drop(resources.logical_resource_slot);
        if let (Some(runtime), Some(handle)) = (self.runtime.clone(), resources.runtime_handle) {
            std::thread::spawn(move || {
                if let Err(err) = runtime.stop(&handle, CONTAINER_STOP_TIMEOUT) {
                    log::warn!(
                        "[container-transport] {} stop failed for session {}: {}",
                        reason,
                        handle.session_id,
                        err
                    );
                }
            });
        }
    }

    fn cleanup_removed_resources_blocking(&self, resources: RemovedSessionResources) {
        if let (Some(manager), Some(client_id)) =
            (self.token_manager.clone(), resources.api_client_id.clone())
        {
            manager.revoke(&client_id);
        }
        drop(resources.logical_resource_slot);
        if let (Some(runtime), Some(handle)) = (self.runtime.clone(), resources.runtime_handle) {
            if let Err(err) = runtime.stop(&handle, CONTAINER_STOP_TIMEOUT) {
                log::warn!(
                    "[container-transport] blocking stop failed for session {}: {}",
                    handle.session_id,
                    err
                );
            }
        }
    }

    async fn cleanup_removed_resources_offloaded(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) {
        let token_manager = self.token_manager.clone();
        let runtime = self.runtime.clone();
        let task = tokio::task::spawn_blocking(move || {
            cleanup_removed_resources_inner(token_manager, runtime, resources, reason)
        });
        match tokio::time::timeout(CLEANUP_TASK_TIMEOUT, task).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                log::warn!(
                    "[container-transport] {} cleanup task failed: {}",
                    reason,
                    err
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
                let mgr = self.session_mgr.read().await;
                let _ = mgr.mark_exited(session_id, TRANSPORT_LOST_EXIT_CODE).await;
                crate::config::sessions_persistence::persist_current_state(&mgr).await;
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

    pub fn cleanup_labeled_orphans_on_startup(&self) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let token_manager = self.token_manager.clone();
        std::thread::spawn(move || {
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
                    log::warn!(
                        "[container-transport] startup orphan cleanup failed: {}",
                        err
                    );
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

    pub fn stop_all_started_containers_blocking(&self, budget: Duration) {
        let session_ids = {
            let sessions = self.sessions.lock().unwrap();
            sessions.keys().copied().collect::<Vec<_>>()
        };
        let deadline = Instant::now() + budget;
        for session_id in session_ids {
            let Some(resources) = self.remove_session_state(session_id) else {
                continue;
            };
            self.remove_route(session_id);
            self.cleanup_removed_resources_blocking(resources);
            if Instant::now() >= deadline {
                log::warn!("[container-transport] shutdown container cleanup budget exhausted");
                break;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn last_issued_ticket_for_test(&self, session_id: Uuid) -> Option<String> {
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
    }
}

fn cleanup_removed_resources_inner(
    token_manager: Option<ContainerApiTokenManager>,
    runtime: Option<Arc<dyn ContainerRuntime>>,
    resources: RemovedSessionResources,
    reason: &'static str,
) {
    if let (Some(manager), Some(client_id)) = (token_manager, resources.api_client_id.clone()) {
        manager.revoke(&client_id);
    }
    drop(resources.logical_resource_slot);
    if let (Some(runtime), Some(handle)) = (runtime, resources.runtime_handle) {
        if let Err(err) = runtime.stop(&handle, CONTAINER_STOP_TIMEOUT) {
            log::warn!(
                "[container-transport] {} stop failed for session {}: {}",
                reason,
                handle.session_id,
                err
            );
        }
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
                )?;
                Ok(())
            }
        })
    }

    fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError> {
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

fn resources_from_state(state: ContainerSessionState) -> RemovedSessionResources {
    match state {
        ContainerSessionState::Pending(pending) => RemovedSessionResources {
            runtime_handle: pending.runtime_handle,
            api_client_id: pending.api_client_id,
            logical_resource_slot: pending.logical_resource_slot,
        },
        ContainerSessionState::Attaching(attaching) => RemovedSessionResources {
            runtime_handle: attaching.runtime_handle,
            api_client_id: attaching.api_client_id,
            logical_resource_slot: attaching.logical_resource_slot,
        },
        ContainerSessionState::Active(active) => RemovedSessionResources {
            runtime_handle: active.runtime_handle,
            api_client_id: active.api_client_id,
            logical_resource_slot: active.logical_resource_slot,
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
    registration_ticket: String,
    token: &ContainerApiToken,
) -> Result<ContainerStartRequest, AppError> {
    let settings = crate::config::settings::load_settings();
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
        registration_ticket,
        token,
        &settings,
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
    registration_ticket: String,
    token: &ContainerApiToken,
    settings: &crate::config::settings::AppSettings,
) -> Result<ContainerStartRequest, AppError> {
    if !settings.api_server_enabled {
        return Err(AppError::Other(
            "container transport requires the control-plane API server to be enabled".to_string(),
        ));
    }
    if let Some(reason) =
        crate::pty::container_paths::container_mount_source_rejection(std::path::Path::new(cwd))
    {
        return Err(AppError::Other(reason));
    }
    let api_url = api_url_for_container(&settings.api_server_bind, settings.api_server_port)?;
    let child_env = sanitized_child_env(configured_env, env_remove_keys);
    log::info!(
        "[container-transport] mount source '{}' -> '{}'",
        cwd,
        DEFAULT_CONTAINER_WORKDIR
    );
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
                    if !env_unset.iter().any(|existing| existing == canonical_key) {
                        env_unset.push(canonical_key.to_string());
                    }
                    if canonical_key == CLAUDE_CONFIG_DIR_KEY {
                        warnings.push(claude_config_dir_unmappable_warning(map, &value));
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
    use crate::pty::container_runtime::ContainerCleanupReport;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingRuntime {
        stopped: Arc<Mutex<Vec<Uuid>>>,
    }

    impl RecordingRuntime {
        fn stopped(&self) -> Vec<Uuid> {
            self.stopped.lock().unwrap().clone()
        }
    }

    impl ContainerRuntime for RecordingRuntime {
        fn start(
            &self,
            request: ContainerStartRequest,
        ) -> Result<ContainerRuntimeHandle, AppError> {
            Ok(ContainerRuntimeHandle {
                session_id: request.session_id,
                container_id: format!("container-{}", request.session_id),
            })
        }

        fn stop(
            &self,
            handle: &ContainerRuntimeHandle,
            _timeout: Duration,
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
                session_mgr.clone(),
                tuning,
            ),
            session_mgr,
        )
    }

    fn test_spec(id: Uuid, root: &str, output_target: PtyOutputTarget) -> BackendSpawnSpec {
        BackendSpawnSpec {
            id,
            cmd: "container".to_string(),
            args: Vec::new(),
            cwd: root.to_string(),
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
    async fn expired_pending_reaper_marks_session_exited_and_removes_route() {
        let id_root = "C:/repo/.ac/wg-1/__agent_dev";
        let tuning = ContainerTransportTuning {
            ticket_ttl: Duration::from_millis(1),
            ..ContainerTransportTuning::default()
        };
        let (backend, session_mgr) = backend_with_tuning(tuning);
        let removed = Arc::new(AtomicUsize::new(0));
        let removed_for_cb = removed.clone();
        backend.set_route_remover(Arc::new(move |_| {
            removed_for_cb.fetch_add(1, Ordering::SeqCst);
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
        let stored = session_mgr
            .read()
            .await
            .get_session(session.id)
            .await
            .expect("stored");
        assert!(matches!(
            stored.status,
            crate::session::session::SessionStatus::Exited(TRANSPORT_LOST_EXIT_CODE)
        ));
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
                logical_resource_slot: None,
            }),
        );

        backend.kill(id).expect("kill");

        let registry = crate::api::auth::list(token_manager.path());
        assert!(
            registry
                .clients
                .iter()
                .find(|client| client.client_id == token.client_id)
                .expect("client")
                .revoked
        );
        for _ in 0..20 {
            if runtime.stopped().contains(&id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("runtime stop was not called");
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
        assert_eq!(got.warnings.len(), 1);
        assert_eq!(
            got.warnings[0].kind,
            crate::pty::container_paths::WARNING_KIND_OUTSIDE_MOUNT
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
            secret: "secret".to_string(),
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
            "ticket".to_string(),
            &token(),
            &api_enabled_settings(),
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

        backend.write(id, b"abc").expect("write");

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
    async fn exit_and_disconnect_cleanup_mark_session_exited_and_remove_route() {
        let root = "C:/repo/.ac/wg-1/__agent_dev";
        for code in [7, TRANSPORT_LOST_EXIT_CODE] {
            let (backend, session_mgr) = backend_with_tuning(ContainerTransportTuning::default());
            let removed = Arc::new(AtomicUsize::new(0));
            let removed_for_cb = removed.clone();
            backend.set_route_remover(Arc::new(move |_| {
                removed_for_cb.fetch_add(1, Ordering::SeqCst);
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
            let stored = session_mgr
                .read()
                .await
                .get_session(session.id)
                .await
                .expect("stored");
            assert!(matches!(
                stored.status,
                crate::session::session::SessionStatus::Exited(observed) if observed == code
            ));
        }
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
}
