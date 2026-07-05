//! `POST /api/v1/send` (#791 §6.1). Mirrors `send --mode wake --send <file>`,
//! MINUS `--command` (out of scope). Identity is the token's bound replica,
//! never client input; `inline` is rejected in v1; replays are deduped.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::actuation::{self, DeliveryOutcome};
use crate::api::auth::SCOPE_SEND;
use crate::api::error::{scrub_host_paths, ApiError};
use crate::api::idempotency::StoredResult;
use crate::api::schema::{SendRequest, SendResponse, API_VERSION};
use crate::api::{handlers, ApiState};

/// Max accepted request body (16 KB). Large content belongs in the `.md` file,
/// referenced by basename, never inlined.
const MAX_BODY_BYTES: usize = 16 * 1024;

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
        return Err(ApiError::BadRequest(
            "request body too large (>16 KB); put large content in the .md file".to_string(),
        ));
    }

    // deny_unknown_fields rejects forbidden identity fields at parse time.
    let req: SendRequest = serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid request body: {}", e)))?;

    // Auth + scope. Identity comes from the token, never the body.
    let client = handlers::authenticate(state, headers, ip, SCOPE_SEND)?;
    let from = crate::api::identity::resolve_from(&client)?;

    // Idempotency: a replayed opId returns the SAME stored result, never
    // re-delivers (across restarts, since the ledger is disk-persisted).
    if let Some(prev) = state.ledger.get(&req.op_id) {
        return Ok(replay(prev));
    }

    // Message one-of: `inline` is reserved but rejected in v1; `send` required.
    if req.message.inline.is_some() {
        return Err(ApiError::BadRequest(
            "inline messages are not supported in v1; write the .md into messaging/ and use `send`"
                .to_string(),
        ));
    }
    let basename = req.message.send.as_deref().ok_or_else(|| {
        ApiError::BadRequest("message.send (a bare filename) is required".to_string())
    })?;

    // Build the host-absolute notification body (path confinement inside).
    let notif = actuation::build_send_body(&client.bound_root, basename, &from)?;

    // Resolve + route + actuate through the shared engine.
    let outcome =
        actuation::deliver_wake_via_api(&state.app_handle, &from, &req.to, notif, &req.op_id)
            .await?;

    match outcome {
        DeliveryOutcome::Delivered { to } => {
            state.ledger.put(&req.op_id, "delivered", &to, None);
            crate::api::audit::record(&client.client_id, &from, "send", "delivered");
            Ok((StatusCode::OK, SendResponse::delivered(&req.op_id, &to)))
        }
        DeliveryOutcome::Rejected { to, reason } => {
            let detail = scrub_host_paths(&reason);
            state
                .ledger
                .put(&req.op_id, "rejected", &to, Some(detail.clone()));
            crate::api::audit::record(&client.client_id, &from, "send", "rejected");
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                SendResponse {
                    api_version: API_VERSION.to_string(),
                    op_id: req.op_id.clone(),
                    status: "rejected".to_string(),
                    to,
                    detail: Some(detail),
                },
            ))
        }
    }
}

/// Map a stored (replayed) result back to a status + response body.
fn replay(prev: StoredResult) -> (StatusCode, SendResponse) {
    let status_code = if prev.status == "delivered" {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    (
        status_code,
        SendResponse {
            api_version: API_VERSION.to_string(),
            op_id: prev.op_id,
            status: prev.status,
            to: prev.to,
            detail: prev.detail,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_delivered_is_200() {
        let (code, resp) = replay(StoredResult {
            op_id: "o".into(),
            status: "delivered".into(),
            to: "proj/agent".into(),
            detail: None,
            first_seen: "2026-01-01T00:00:00Z".into(),
        });
        assert_eq!(code, StatusCode::OK);
        assert_eq!(resp.status, "delivered");
        assert!(resp.detail.is_none());
    }

    #[test]
    fn replay_rejected_is_422_with_detail() {
        let (code, resp) = replay(StoredResult {
            op_id: "o".into(),
            status: "rejected".into(),
            to: "proj/agent".into(),
            detail: Some("no route".into()),
            first_seen: "2026-01-01T00:00:00Z".into(),
        });
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(resp.status, "rejected");
        assert_eq!(resp.detail.as_deref(), Some("no route"));
    }
}
