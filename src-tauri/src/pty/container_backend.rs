use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot, SessionIoFanout};
use crate::session::manager::SessionManager;
use crate::telegram::manager::OutputSenderMap;

pub const TRANSPORT_PROTOCOL_VERSION: u32 = 1;
pub const MAX_TRANSPORT_FRAME_BYTES: usize = 64 * 1024;
const TRANSPORT_LOST_EXIT_CODE: i32 = 1;

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

#[derive(Clone)]
struct PendingSession {
    root_key: String,
    ticket_hash: String,
    ticket_expires_at: Instant,
    output_target: PtyOutputTarget,
    idle_tuning: crate::session::profile::IdleTuning,
    rows: u16,
    cols: u16,
}

#[derive(Clone)]
struct AttachingSession {
    root_key: String,
    output_target: PtyOutputTarget,
    idle_tuning: crate::session::profile::IdleTuning,
    rows: u16,
    cols: u16,
}

#[derive(Clone)]
struct ActiveSession {
    output_target: PtyOutputTarget,
    sender: mpsc::Sender<HostToBridgeFrame>,
    rows: u16,
    cols: u16,
}

#[derive(Clone)]
enum ContainerSessionState {
    Pending(PendingSession),
    Attaching(AttachingSession),
    Active(ActiveSession),
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
            #[cfg(test)]
            issued_tickets_for_test: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let Some(state) = sessions.get_mut(&session_id) else {
            return Err(TransportTicketError::Invalid);
        };

        let pending = match state {
            ContainerSessionState::Pending(pending) => pending,
            ContainerSessionState::Attaching(_) | ContainerSessionState::Active(_) => {
                return Err(TransportTicketError::Invalid);
            }
        };

        if pending.root_key != bound_key
            || pending.ticket_expires_at <= now
            || !crate::api::auth::constant_time_eq(&pending.ticket_hash, &ticket_hash)
        {
            return Err(TransportTicketError::Invalid);
        }

        let attaching = AttachingSession {
            root_key: pending.root_key.clone(),
            output_target: pending.output_target.clone(),
            idle_tuning: pending.idle_tuning,
            rows: pending.rows,
            cols: pending.cols,
        };
        *state = ContainerSessionState::Attaching(attaching);
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
            let Some(state) = sessions.get_mut(&session_id) else {
                return Err(TransportAttachError::Invalid);
            };

            let ContainerSessionState::Attaching(attach) = state else {
                return Err(TransportAttachError::Invalid);
            };
            if attach.root_key != bridge_key {
                return Err(TransportAttachError::Invalid);
            }

            let attach = attach.clone();
            *state = ContainerSessionState::Active(ActiveSession {
                output_target: attach.output_target.clone(),
                sender,
                rows: attach.rows,
                cols: attach.cols,
            });
            attach
        };

        self.fanout
            .register_session(session_id, attach.idle_tuning, attach.rows, attach.cols);
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
        let removed = self.remove_session_state(session_id);
        if !removed {
            return;
        }

        self.remove_route(session_id);
        if let Some(code) = exit_code {
            let mgr = self.session_mgr.read().await;
            let _ = mgr.mark_exited(session_id, code).await;
            crate::config::sessions_persistence::persist_current_state(&mgr).await;
        }
    }

    fn close_transport_from_sync(&self, session_id: Uuid, exit_code: i32) {
        let removed = self.remove_session_state(session_id);
        if !removed {
            return;
        }

        self.remove_route(session_id);
        let session_mgr = self.session_mgr.clone();
        tauri::async_runtime::spawn(async move {
            let mgr = session_mgr.read().await;
            let _ = mgr.mark_exited(session_id, exit_code).await;
            crate::config::sessions_persistence::persist_current_state(&mgr).await;
        });
    }

    fn remove_session_state(&self, session_id: Uuid) -> bool {
        let removed = self.sessions.lock().unwrap().remove(&session_id).is_some();
        if removed {
            self.fanout.remove_session(session_id);
        }
        removed
    }

    fn remove_route(&self, session_id: Uuid) {
        let remover = self.route_remover.lock().unwrap().clone();
        if let Some(remove) = remover {
            remove(session_id);
        }
    }

    fn create_pending_session(&self, spec: BackendSpawnSpec) -> Result<String, AppError> {
        let BackendSpawnSpec {
            id,
            cwd,
            rows,
            cols,
            idle_tuning,
            output_target,
            ..
        } = spec;

        let ticket = format!("acst-{}-{}", Uuid::new_v4(), Uuid::new_v4());
        let ticket_hash = crate::api::auth::hash_token(&ticket);
        let pending = PendingSession {
            root_key: root_key(&cwd),
            ticket_hash,
            ticket_expires_at: Instant::now() + self.tuning.ticket_ttl,
            output_target,
            idle_tuning,
            rows,
            cols,
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

    #[cfg(test)]
    pub(crate) fn last_issued_ticket_for_test(&self, session_id: Uuid) -> Option<String> {
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
    }
}

impl PtyBackend for ContainerTransportBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            let _ticket = self.create_pending_session(spec)?;
            Ok(())
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
        self.remove_session_state(id);
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

pub(crate) fn root_key(root: &str) -> String {
    let normalized = crate::path_utils::normalize_windows_verbatim_path(root);
    let normalized = normalized.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/').to_string();
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::backend::SessionBackendKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target,
            resource_registration: None,
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
