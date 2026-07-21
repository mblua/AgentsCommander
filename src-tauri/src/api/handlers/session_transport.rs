//! `GET /api/v1/session-transport`. Authenticates a bridge before WebSocket
//! upgrade, atomically consumes its one-time session ticket, then binds the
//! bridge to the container PTY backend.

use std::net::SocketAddr;
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::auth::SCOPE_SESSION_TRANSPORT;
use crate::api::error::ApiError;
use crate::api::{handlers, ApiState};
use crate::pty::container_backend::{
    parse_bridge_text_frame, root_key, BridgeToHostFrame, ContainerTransportBackend,
    HostToBridgeFrame, MAX_TRANSPORT_FRAME_BYTES, TRANSPORT_PROTOCOL_VERSION,
};
use crate::session::warnings::{
    emit_session_warning, SessionWarning, CONTAINER_TRANSPORT_PROTOCOL_KEY,
    WARNING_KIND_PROTOCOL_MISMATCH,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTransportQuery {
    session_id: Uuid,
    ticket: String,
}

struct AuthorizedTransport {
    backend: std::sync::Arc<ContainerTransportBackend>,
    session_id: Uuid,
    bound_root: String,
}

pub async fn handle(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<SessionTransportQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorized = match authorize_transport(&state, addr.ip(), &headers, query).await {
        Ok(authorized) => authorized,
        Err(err) => return err.into_response(),
    };

    let app_handle = state.app_handle.clone();
    ws.max_message_size(MAX_TRANSPORT_FRAME_BYTES)
        .max_frame_size(MAX_TRANSPORT_FRAME_BYTES)
        .on_upgrade(move |socket| {
            handle_ws_connection(
                socket,
                authorized.backend,
                authorized.session_id,
                authorized.bound_root,
                app_handle,
            )
        })
}

async fn authorize_transport(
    state: &ApiState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    query: SessionTransportQuery,
) -> Result<AuthorizedTransport, ApiError> {
    state.lockout.check(ip).map_err(|error| {
        if error.status() == StatusCode::TOO_MANY_REQUESTS {
            error
        } else {
            uniform_transport_auth_failure()
        }
    })?;
    let presented = match handlers::bearer_token_strict(headers) {
        Ok(presented) => presented,
        Err(_) => return Err(reject_transport_auth(state, ip, None, "missing_token")),
    };
    let fresh = match classify_fresh_transport_auth(
        state
            .store
            .authenticate_pty_input_fresh_offloaded(presented)
            .await,
    ) {
        FreshTransportAuth::Authenticated(fresh) => *fresh,
        FreshTransportAuth::InvalidToken => {
            return Err(reject_transport_auth(state, ip, None, "invalid_token"))
        }
        FreshTransportAuth::DependencyUnavailable => return Err(transport_dependency_unavailable()),
    };
    let client = &fresh.client;
    let session_id_text = query.session_id.to_string();
    let generation = match client.credential_generation.as_deref() {
        Some(generation) => generation,
        None => {
            return Err(reject_transport_auth(
                state,
                ip,
                Some(client),
                "unbound_client",
            ));
        }
    };
    if !client.has_scope(SCOPE_SESSION_TRANSPORT) {
        return Err(reject_transport_auth(
            state,
            ip,
            Some(client),
            "forbidden_scope",
        ));
    }
    if client.bound_session_id.as_deref() != Some(session_id_text.as_str())
        || crate::api::identity::resolve_from(client).is_err()
    {
        return Err(reject_transport_auth(
            state,
            ip,
            Some(client),
            "binding_mismatch",
        ));
    }

    let backend = {
        let pty_mgr = state.pty_mgr.lock().unwrap();
        pty_mgr.container_backend()
    };

    let binding = match backend.credential_binding(query.session_id) {
        Some(binding) => binding,
        None => {
            return Err(reject_transport_auth(
                state,
                ip,
                Some(client),
                "binding_mismatch",
            ));
        }
    };
    let root =
        match crate::path_identity::verify_directory(std::path::Path::new(&client.bound_root)) {
            Ok(root) => root,
            Err(_) => {
                return Err(reject_transport_auth(
                    state,
                    ip,
                    Some(client),
                    "binding_mismatch",
                ));
            }
        };
    if binding.client_id != client.client_id
        || binding.credential_generation != generation
        || binding.bound_session_id != query.session_id.to_string()
        || binding.bound_root_object_id != root.object_id
        || !crate::api::auth::constant_time_eq(
            &binding.credential_token_hash,
            &fresh.presented_token_hash,
        )
    {
        return Err(reject_transport_auth(
            state,
            ip,
            Some(client),
            "binding_mismatch",
        ));
    }
    if backend
        .consume_ticket(query.session_id, &client.bound_root, &query.ticket)
        .is_err()
    {
        return Err(reject_transport_auth(
            state,
            ip,
            Some(client),
            "invalid_ticket",
        ));
    }
    if state.lockout.record_success(ip).is_err() {
        log::warn!("[container-transport] lockout success update failed code=auth_state_failed");
        return Err(uniform_transport_auth_failure());
    }
    crate::api::audit::record(
        &client.client_id,
        &client.bound_fqn,
        SCOPE_SESSION_TRANSPORT,
        "authenticated",
    );

    Ok(AuthorizedTransport {
        backend,
        session_id: query.session_id,
        bound_root: client.bound_root.clone(),
    })
}

fn reject_transport_auth(
    state: &ApiState,
    ip: std::net::IpAddr,
    client: Option<&crate::api::auth::ApiClient>,
    outcome: &'static str,
) -> ApiError {
    if state.lockout.record_failure(ip).is_err() {
        log::warn!("[container-transport] lockout failure update failed code=auth_state_failed");
    }
    let (client_id, bound_fqn) = client
        .map(|client| (client.client_id.as_str(), client.bound_fqn.as_str()))
        .unwrap_or(("-", "-"));
    crate::api::audit::record(client_id, bound_fqn, SCOPE_SESSION_TRANSPORT, outcome);
    uniform_transport_auth_failure()
}

fn uniform_transport_auth_failure() -> ApiError {
    ApiError::Unauthorized("transport authentication failed".to_string())
}

fn transport_dependency_unavailable() -> ApiError {
    ApiError::ServiceUnavailable("transport authentication dependency unavailable".to_string())
}

/// The transport boundary's decision over the offloaded fresh-registry lookup.
/// A `FreshRegistryError` is a dependency failure, never evidence a credential
/// was revoked, so contention must classify as retry-class service
/// unavailability, not credential failure.
enum FreshTransportAuth {
    Authenticated(Box<crate::api::auth::ApiClientFreshGuard>),
    InvalidToken,
    DependencyUnavailable,
}

/// Classify the offloaded fresh-registry lookup result. Extracted from
/// `authorize_transport` so this exact retry-class-versus-credential-failure
/// decision is unit-testable: `ApiState` carries a concrete Tauri (`Wry`)
/// runtime handle that cannot be built headlessly, so the handler cannot be
/// driven directly from a test. Behavior is identical to the inline match.
fn classify_fresh_transport_auth(
    result: Result<
        Option<crate::api::auth::ApiClientFreshGuard>,
        crate::api::auth::FreshRegistryError,
    >,
) -> FreshTransportAuth {
    match result {
        Ok(Some(fresh)) => FreshTransportAuth::Authenticated(Box::new(fresh)),
        Ok(None) => FreshTransportAuth::InvalidToken,
        Err(
            crate::api::auth::FreshRegistryError::Contended
            | crate::api::auth::FreshRegistryError::Internal,
        ) => FreshTransportAuth::DependencyUnavailable,
    }
}

async fn handle_ws_connection(
    mut socket: WebSocket,
    backend: std::sync::Arc<ContainerTransportBackend>,
    session_id: Uuid,
    bound_root: String,
    app_handle: tauri::AppHandle,
) {
    let first = match tokio::time::timeout(
        backend.tuning().handshake_timeout,
        StreamExt::next(&mut socket),
    )
    .await
    {
        Ok(Some(Ok(message))) => message,
        _ => {
            backend.handle_handshake_failed(session_id).await;
            return;
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel(backend.tuning().outbound_queue_capacity);
    if let Err(rejection) = validate_hello(first, &backend, session_id, &bound_root, tx).await {
        let reason = rejection.message();
        log::error!("[container-transport] {reason}");
        if let Some(warning) = rejection.session_warning(session_id) {
            emit_session_warning(&app_handle, warning);
        }
        backend.handle_handshake_failed(session_id).await;
        return;
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut outbound_rx = rx;
    let mut send_task = tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            let message = match frame {
                HostToBridgeFrame::Text(frame) => match serde_json::to_string(&frame) {
                    Ok(text) => Message::Text(text.into()),
                    Err(err) => {
                        log::warn!("[container-transport] failed to serialize host frame: {err}");
                        break;
                    }
                },
                HostToBridgeFrame::Binary(data) => Message::Binary(data.into()),
            };
            if SinkExt::send(&mut ws_sender, message).await.is_err() {
                break;
            }
        }
    });

    let recv_backend = backend.clone();
    let mut recv_task = tokio::spawn(async move {
        recv_bridge_loop(&recv_backend, session_id, &mut ws_receiver).await;
    });

    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
            let _ = recv_task.await;
        }
        _ = &mut recv_task => {
            send_task.abort();
            let _ = send_task.await;
        }
    }

    backend.handle_bridge_disconnect(session_id).await;
}

async fn validate_hello(
    message: Message,
    backend: &ContainerTransportBackend,
    session_id: Uuid,
    bound_root: &str,
    sender: tokio::sync::mpsc::Sender<HostToBridgeFrame>,
) -> Result<(), HelloRejection> {
    let Message::Text(text) = message else {
        return Err(HelloRejection::FirstFrameNotText);
    };
    if text.len() > MAX_TRANSPORT_FRAME_BYTES {
        return Err(HelloRejection::FirstFrameTooLarge);
    }

    let frame = parse_bridge_text_frame(text.as_str()).map_err(|_| HelloRejection::InvalidHello)?;
    let BridgeToHostFrame::Hello {
        version,
        session_id: hello_session_id,
        root,
    } = frame
    else {
        return Err(HelloRejection::FirstFrameNotHello);
    };

    if version != TRANSPORT_PROTOCOL_VERSION {
        return Err(HelloRejection::ProtocolMismatch {
            expected: TRANSPORT_PROTOCOL_VERSION,
            reported: version,
        });
    }

    if hello_session_id != session_id {
        return Err(HelloRejection::SessionIdMismatch {
            expected: session_id,
            reported: hello_session_id,
        });
    }

    if root_key(&root) != root_key(bound_root) {
        return Err(HelloRejection::RootMismatch);
    }

    backend
        .complete_hello(session_id, &root, sender)
        .map_err(|_| HelloRejection::SessionNotPending)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelloRejection {
    FirstFrameNotText,
    FirstFrameTooLarge,
    InvalidHello,
    FirstFrameNotHello,
    ProtocolMismatch { expected: u32, reported: u32 },
    SessionIdMismatch { expected: Uuid, reported: Uuid },
    RootMismatch,
    SessionNotPending,
}

impl HelloRejection {
    fn message(&self) -> String {
        match self {
            Self::FirstFrameNotText => {
                "container transport handshake failed: first frame was not text".to_string()
            }
            Self::FirstFrameTooLarge => {
                "container transport handshake failed: first frame was too large".to_string()
            }
            Self::InvalidHello => {
                "container transport handshake failed: invalid hello".to_string()
            }
            Self::FirstFrameNotHello => {
                "container transport handshake failed: first frame was not hello".to_string()
            }
            Self::ProtocolMismatch { expected, reported } => format!(
                "container transport protocol mismatch: host expects protocol {} but container image reported protocol {}. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.",
                expected, reported
            ),
            Self::SessionIdMismatch { expected, reported } => format!(
                "container transport handshake failed: hello session id {} did not match expected {}",
                reported, expected
            ),
            Self::RootMismatch => {
                "container transport handshake failed: bridge root did not match bound root"
                    .to_string()
            }
            Self::SessionNotPending => {
                "container transport handshake failed: session was not pending".to_string()
            }
        }
    }

    fn session_warning(&self, session_id: Uuid) -> Option<SessionWarning> {
        match self {
            Self::ProtocolMismatch { .. } => Some(SessionWarning::new(
                session_id,
                CONTAINER_TRANSPORT_PROTOCOL_KEY,
                WARNING_KIND_PROTOCOL_MISMATCH,
                self.message(),
            )),
            _ => None,
        }
    }
}

async fn recv_bridge_loop(
    backend: &ContainerTransportBackend,
    session_id: Uuid,
    ws_receiver: &mut futures_util::stream::SplitStream<WebSocket>,
) {
    let mut last_seen = Instant::now();
    let mut heartbeat = tokio::time::interval(backend.tuning().heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > backend.tuning().max_idle {
                    break;
                }
                if backend.send_ping(session_id).is_err() {
                    break;
                }
            }
            message = StreamExt::next(ws_receiver) => {
                let Some(Ok(message)) = message else {
                    break;
                };
                last_seen = Instant::now();
                if !handle_bridge_message(backend, session_id, message).await {
                    break;
                }
            }
        }
    }
}

async fn handle_bridge_message(
    backend: &ContainerTransportBackend,
    session_id: Uuid,
    message: Message,
) -> bool {
    match message {
        Message::Binary(data) => {
            if data.len() > MAX_TRANSPORT_FRAME_BYTES {
                log::warn!(
                    "[container-transport] closing session {} after oversized binary frame",
                    session_id
                );
                return false;
            }
            backend
                .handle_bridge_output(session_id, data.to_vec())
                .is_ok()
        }
        Message::Text(text) => {
            if text.len() > MAX_TRANSPORT_FRAME_BYTES {
                log::warn!(
                    "[container-transport] closing session {} after oversized text frame",
                    session_id
                );
                return false;
            }

            let Ok(frame) = parse_bridge_text_frame(text.as_str()) else {
                return false;
            };
            if frame.version() != TRANSPORT_PROTOCOL_VERSION {
                log::warn!(
                    "[container-transport] closing session {} after protocol mismatch: host expects protocol {} but bridge sent protocol {}. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.",
                    session_id,
                    TRANSPORT_PROTOCOL_VERSION,
                    frame.version()
                );
                return false;
            }

            match frame {
                BridgeToHostFrame::Hello { .. } => false,
                BridgeToHostFrame::Status { status, .. } => {
                    if let Some(status) = status {
                        log::debug!(
                            "[container-transport] status frame for session {} ({} chars)",
                            session_id,
                            status.len()
                        );
                    }
                    true
                }
                BridgeToHostFrame::Pong { .. } => true,
                BridgeToHostFrame::Exit { code, .. } => {
                    backend.handle_bridge_exit(session_id, code).await;
                    false
                }
            }
        }
        Message::Ping(_) | Message::Pong(_) => true,
        Message::Close(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::{ApiClientStore, FreshRegistryError, REGISTRY_FILENAME};
    use crate::pty::idle_detector::IdleDetector;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn uniform_transport_auth_failure_is_generic_401() {
        let response = uniform_transport_auth_failure().into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn transport_dependency_unavailable_is_retry_class_503() {
        let response = transport_dependency_unavailable().into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn fresh_registry_dependency_failures_are_retry_class_not_credential_failures() {
        // A dependency failure of the privileged registry lookup (contention or
        // an internal error) must never be reported to a container bridge as a
        // credential failure. Both classify as retry-class unavailability.
        assert!(matches!(
            classify_fresh_transport_auth(Err(FreshRegistryError::Contended)),
            FreshTransportAuth::DependencyUnavailable
        ));
        assert!(matches!(
            classify_fresh_transport_auth(Err(FreshRegistryError::Internal)),
            FreshTransportAuth::DependencyUnavailable
        ));
        // A registry that is readable but holds no matching credential is a real
        // credential failure.
        assert!(matches!(
            classify_fresh_transport_auth(Ok(None)),
            FreshTransportAuth::InvalidToken
        ));
    }

    #[tokio::test]
    async fn held_registry_lock_makes_transport_auth_retry_class_with_executor_progress() {
        let dir = tempfile::TempDir::new().unwrap();
        // Hold the dedicated registry lock so the offloaded fresh lookup that the
        // transport boundary awaits observes cross-process contention.
        let _held = crate::api::auth::hold_registry_lock_for_test(dir.path());
        let store = Arc::new(ApiClientStore::new(dir.path().join(REGISTRY_FILENAME)));

        let lookup = {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                store
                    .authenticate_pty_input_fresh_offloaded("presented-token".to_string())
                    .await
            })
        };

        // The async executor must keep running while the blocking registry lookup
        // polls the held lock on the blocking pool.
        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        })
        .await
        .expect("the async executor must progress while the registry lock is held");

        let result = lookup.await.unwrap();
        assert!(
            matches!(&result, Err(FreshRegistryError::Contended)),
            "a held registry lock must surface as retry-class contention, not a missing credential"
        );

        // The transport boundary classifies that contention as retry-class service
        // unavailability (503), never credential failure (401).
        assert!(matches!(
            classify_fresh_transport_auth(result),
            FreshTransportAuth::DependencyUnavailable
        ));
        assert_eq!(
            transport_dependency_unavailable().into_response().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn validate_hello_protocol_mismatch_names_image_remedy() {
        let output_senders: crate::telegram::manager::OutputSenderMap =
            Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let backend = ContainerTransportBackend::new(output_senders, idle_detector, None, None);
        let session_id = Uuid::new_v4();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let reported_version = TRANSPORT_PROTOCOL_VERSION - 1;
        let text = serde_json::json!({
            "type": "hello",
            "version": reported_version,
            "sessionId": session_id,
            "root": "C:/root"
        })
        .to_string();

        let err = validate_hello(
            Message::Text(text.into()),
            &backend,
            session_id,
            "C:/root",
            tx,
        )
        .await
        .expect_err("protocol mismatch");

        assert!(matches!(
            err,
            HelloRejection::ProtocolMismatch {
                expected: TRANSPORT_PROTOCOL_VERSION,
                reported
            } if reported == reported_version
        ));
        assert_eq!(
            err.message(),
            format!(
                "container transport protocol mismatch: host expects protocol {} but container image reported protocol {}. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.",
                TRANSPORT_PROTOCOL_VERSION, reported_version
            )
        );
    }

    #[test]
    fn protocol_mismatch_warning_payload_uses_session_env_warning_contract() {
        let session_id = Uuid::nil();
        let rejection = HelloRejection::ProtocolMismatch {
            expected: 2,
            reported: 1,
        };

        let warning = rejection
            .session_warning(session_id)
            .expect("protocol mismatch warning");

        assert_eq!(warning.session_id, session_id.to_string());
        assert_eq!(warning.key, CONTAINER_TRANSPORT_PROTOCOL_KEY);
        assert_eq!(warning.kind, WARNING_KIND_PROTOCOL_MISMATCH);
        assert_eq!(
            warning.message,
            "container transport protocol mismatch: host expects protocol 2 but container image reported protocol 1. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge."
        );
    }
}
