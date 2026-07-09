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
use tauri::Emitter;
use uuid::Uuid;

use crate::api::auth::SCOPE_SESSION_TRANSPORT;
use crate::api::error::ApiError;
use crate::api::{handlers, ApiState};
use crate::pty::container_backend::{
    parse_bridge_text_frame, root_key, BridgeToHostFrame, ContainerTransportBackend,
    HostToBridgeFrame, MAX_TRANSPORT_FRAME_BYTES, TRANSPORT_PROTOCOL_VERSION,
};
use crate::pty::container_paths::WARNING_KIND_PROTOCOL_MISMATCH;

const PROTOCOL_WARNING_KEY: &str = "CONTAINER_TRANSPORT_PROTOCOL";

#[derive(Debug, Deserialize)]
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
    let authorized = match authorize_transport(&state, addr.ip(), &headers, query) {
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

fn authorize_transport(
    state: &ApiState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    query: SessionTransportQuery,
) -> Result<AuthorizedTransport, ApiError> {
    let client = match handlers::authenticate(state, headers, ip, SCOPE_SESSION_TRANSPORT) {
        Ok(client) => client,
        Err(err) if err.status() == StatusCode::TOO_MANY_REQUESTS => return Err(err),
        Err(_) => return Err(uniform_transport_auth_failure()),
    };

    if crate::api::identity::resolve_from(&client).is_err() {
        return Err(uniform_transport_auth_failure());
    }

    let backend = {
        let pty_mgr = state.pty_mgr.lock().unwrap();
        pty_mgr.container_backend()
    };

    if backend
        .consume_ticket(query.session_id, &client.bound_root, &query.ticket)
        .is_err()
    {
        return Err(uniform_transport_auth_failure());
    }

    Ok(AuthorizedTransport {
        backend,
        session_id: query.session_id,
        bound_root: client.bound_root,
    })
}

fn uniform_transport_auth_failure() -> ApiError {
    ApiError::Unauthorized("transport authentication failed".to_string())
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
    if let Err(reason) = validate_hello(first, &backend, session_id, &bound_root, tx).await {
        log::error!("[container-transport] {reason}");
        if reason.starts_with("container transport protocol mismatch:") {
            emit_session_env_warning(
                &app_handle,
                protocol_mismatch_session_warning_payload(session_id, &reason),
            );
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
) -> Result<(), String> {
    let Message::Text(text) = message else {
        return Err("container transport handshake failed: first frame was not text".to_string());
    };
    if text.len() > MAX_TRANSPORT_FRAME_BYTES {
        return Err("container transport handshake failed: first frame was too large".to_string());
    }

    let frame = parse_bridge_text_frame(text.as_str())
        .map_err(|e| format!("container transport handshake failed: invalid hello: {e}"))?;
    let BridgeToHostFrame::Hello {
        version,
        session_id: hello_session_id,
        root,
    } = frame
    else {
        return Err("container transport handshake failed: first frame was not hello".to_string());
    };

    if version != TRANSPORT_PROTOCOL_VERSION {
        return Err(format!(
            "container transport protocol mismatch: host expects protocol {} but container image reported protocol {}. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.",
            TRANSPORT_PROTOCOL_VERSION, version
        ));
    }

    if hello_session_id != session_id {
        return Err(format!(
            "container transport handshake failed: hello session id {} did not match expected {}",
            hello_session_id, session_id
        ));
    }

    if root_key(&root) != root_key(bound_root) {
        return Err(
            "container transport handshake failed: bridge root did not match bound root"
                .to_string(),
        );
    }

    backend
        .complete_hello(session_id, &root, sender)
        .map_err(|_| "container transport handshake failed: session was not pending".to_string())
}

fn protocol_mismatch_session_warning_payload(session_id: Uuid, message: &str) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id.to_string(),
        "key": PROTOCOL_WARNING_KEY,
        "kind": WARNING_KIND_PROTOCOL_MISMATCH,
        "message": message,
    })
}

fn emit_session_env_warning(app_handle: &tauri::AppHandle, payload: serde_json::Value) {
    let _ = app_handle.emit("session_env_warning", payload);
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
    use crate::pty::idle_detector::IdleDetector;
    use crate::session::manager::SessionManager;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn uniform_transport_auth_failure_is_generic_401() {
        let response = uniform_transport_auth_failure().into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn validate_hello_protocol_mismatch_names_image_remedy() {
        let output_senders: crate::telegram::manager::OutputSenderMap =
            Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend =
            ContainerTransportBackend::new(output_senders, idle_detector, None, session_mgr);
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

        assert_eq!(
            err,
            format!(
                "container transport protocol mismatch: host expects protocol {} but container image reported protocol {}. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.",
                TRANSPORT_PROTOCOL_VERSION, reported_version
            )
        );
    }

    #[test]
    fn protocol_mismatch_warning_payload_uses_session_env_warning_contract() {
        let session_id = Uuid::nil();
        let message = "container transport protocol mismatch: host expects protocol 2 but container image reported protocol 1. Rebuild or re-pull the AgentsCommander container image so it contains the current session-bridge.";

        let payload = protocol_mismatch_session_warning_payload(session_id, message);

        assert_eq!(payload["sessionId"], session_id.to_string());
        assert_eq!(payload["key"], PROTOCOL_WARNING_KEY);
        assert_eq!(payload["kind"], WARNING_KIND_PROTOCOL_MISMATCH);
        assert_eq!(payload["message"], message);
    }
}
