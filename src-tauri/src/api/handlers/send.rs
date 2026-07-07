//! `POST /api/v1/send` (#791 + #833). Identity is the token's bound replica,
//! never client input. The handler queues inline content durably and the
//! dispatcher performs delivery.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::actuation;
use crate::api::auth::SCOPE_SEND;
use crate::api::error::ApiError;
use crate::api::message_store::{
    EnqueueRequest, MessageStoreError, DEFAULT_CONTENT_TYPE, INLINE_BODY_MAX_BYTES,
};
use crate::api::schema::{SendRequest, SendResponse};
use crate::api::{handlers, ApiState};

/// Max buffered JSON request body: inline cap plus envelope overhead.
const MAX_BODY_BYTES: usize = INLINE_BODY_MAX_BYTES + (16 * 1024);

/// Axum entry point. Buffers the body (bounded by a router layer), then defers
/// to `handle_inner`, mapping its `(StatusCode, SendResponse)` or `ApiError`.
pub async fn handle(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match handle_inner(&state, addr.ip(), &headers, &body).await {
        Ok((status, resp)) => (status, Json(resp)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn handle_inner(
    state: &ApiState,
    ip: std::net::IpAddr,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(StatusCode, SendResponse), ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::PayloadTooLarge(format!(
            "request body too large (>{} bytes)",
            MAX_BODY_BYTES
        )));
    }

    // deny_unknown_fields rejects forbidden identity fields at parse time.
    let req: SendRequest = serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid request body: {}", e)))?;

    // Auth + scope. Identity comes from the token, never the body.
    let client = handlers::authenticate(state, headers, ip, SCOPE_SEND)?;
    let from = crate::api::identity::resolve_from(&client)?;

    let target = actuation::resolve_api_send_target(&state.app_handle, &from, &req.to).await?;
    let content_type = req
        .message
        .content_type
        .clone()
        .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string());
    let payload = extract_payload(&client.bound_root, &req)?;

    let result = state
        .message_store
        .enqueue_offloaded(EnqueueRequest {
            sender_fqn: from.clone(),
            target_fqn: target.clone(),
            op_id: req.op_id.clone(),
            content_type,
            body: payload.body,
            source_plane: payload.source_plane,
            source_ref: payload.source_ref,
        })
        .await
        .map_err(store_error)?;

    crate::api::audit::record(
        &client.client_id,
        &from,
        "send",
        if result.duplicate {
            "queued_duplicate"
        } else {
            "queued"
        },
    );
    Ok((
        StatusCode::ACCEPTED,
        SendResponse::queued(&result.op_id, &result.target_fqn, &result.message_id),
    ))
}

#[derive(Debug)]
struct Payload {
    body: String,
    source_plane: String,
    source_ref: Option<String>,
}

fn extract_payload(bound_root: &str, req: &SendRequest) -> Result<Payload, ApiError> {
    match (&req.message.send, &req.message.inline) {
        (Some(_), Some(_)) => Err(ApiError::BadRequest(
            "exactly one of message.send or message.inline is required".to_string(),
        )),
        (None, None) => Err(ApiError::BadRequest(
            "exactly one of message.send or message.inline is required".to_string(),
        )),
        (None, Some(inline)) => {
            if inline.len() > INLINE_BODY_MAX_BYTES {
                return Err(ApiError::PayloadTooLarge(format!(
                    "message.inline exceeds inline cap ({} bytes)",
                    INLINE_BODY_MAX_BYTES
                )));
            }
            Ok(Payload {
                body: inline.clone(),
                source_plane: "api_inline".to_string(),
                source_ref: None,
            })
        }
        (Some(basename), None) => {
            let content = actuation::read_send_file_content(bound_root, basename)?;
            Ok(Payload {
                body: content.body,
                source_plane: "api_send_file".to_string(),
                source_ref: Some(content.source_ref),
            })
        }
    }
}

fn store_error(err: MessageStoreError) -> ApiError {
    match err {
        MessageStoreError::BodyTooLarge => ApiError::PayloadTooLarge(format!(
            "message body exceeds inline cap ({} bytes)",
            INLINE_BODY_MAX_BYTES
        )),
        other => ApiError::Internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_payload_accepts_inline() {
        let req: SendRequest = serde_json::from_str(
            r#"{"apiVersion":"1","opId":"o","to":"proj/agent","message":{"inline":"hello"}}"#,
        )
        .unwrap();

        let payload = extract_payload("unused", &req).unwrap();

        assert_eq!(payload.body, "hello");
        assert_eq!(payload.source_plane, "api_inline");
        assert_eq!(payload.source_ref, None);
    }

    #[test]
    fn extract_payload_rejects_both_send_and_inline() {
        let req: SendRequest = serde_json::from_str(
            r#"{"apiVersion":"1","opId":"o","to":"proj/agent","message":{"send":"20260101-000000-wg1-a-to-wg1-b-x.md","inline":"hello"}}"#,
        )
        .unwrap();

        let err = extract_payload("unused", &req).unwrap_err();

        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn extract_payload_rejects_oversize_inline_with_413() {
        let req = SendRequest {
            api_version: crate::api::schema::API_VERSION.to_string(),
            op_id: "o".to_string(),
            to: "proj/agent".to_string(),
            message: crate::api::schema::SendMessage {
                send: None,
                inline: Some("x".repeat(INLINE_BODY_MAX_BYTES + 1)),
                content_type: None,
            },
        };

        let err = extract_payload("unused", &req).unwrap_err();

        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
