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

    let target =
        actuation::resolve_api_send_target(state.resources.as_ref(), &from, &req.to).await?;
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
        .map_err(|error| {
            store_error_with_archive_warning(error, payload.archive_warning.as_deref())
        })?;

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
    Ok(queued_response(
        &result.op_id,
        &result.target_fqn,
        &result.message_id,
        payload.archive_warning,
    ))
}

fn queued_response(
    op_id: &str,
    target: &str,
    message_id: &str,
    warning: Option<String>,
) -> (StatusCode, SendResponse) {
    let mut response = SendResponse::queued(op_id, target, message_id);
    response.detail = warning.map(|warning| crate::api::error::scrub_host_paths(&warning));
    (StatusCode::ACCEPTED, response)
}

#[derive(Debug)]
struct Payload {
    body: String,
    source_plane: String,
    source_ref: Option<String>,
    archive_warning: Option<String>,
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
                archive_warning: None,
            })
        }
        (Some(basename), None) => {
            let content = actuation::read_send_file_content(bound_root, basename)?;
            Ok(Payload {
                body: content.body,
                source_plane: "api_send_file".to_string(),
                source_ref: Some(content.source_ref),
                archive_warning: content.archive_warning,
            })
        }
    }
}

fn store_error_with_archive_warning(error: MessageStoreError, warning: Option<&str>) -> ApiError {
    let mut error = store_error(error);
    if let Some(warning) = warning {
        // Keep the original enqueue status and cause in both store_error variants.
        if let ApiError::PayloadTooLarge(detail) | ApiError::Internal(detail) = &mut error {
            detail.push_str("; ");
            detail.push_str(warning);
        }
    }
    error
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
mod tests;
