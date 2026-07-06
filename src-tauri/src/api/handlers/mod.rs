//! HTTP handlers for the control-plane API (#791 §6, §7).

pub mod list_peers;
pub mod send;
pub mod session_transport;

use std::net::IpAddr;

use axum::http::HeaderMap;
use axum::Json;

use crate::api::auth::ApiClient;
use crate::api::error::ApiError;
use crate::api::schema::HealthResponse;
use crate::api::ApiState;

/// `GET /api/v1/healthz` - unauthenticated liveness. Body pinned to exactly
/// `{"ok":true}` (§0.5 G9): no version, bind, or client count.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

/// Extract a non-empty bearer token from `Authorization: Bearer <token>`.
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Full auth pipeline shared by every authenticated endpoint:
/// lockout check (429) -> bearer extraction -> read-through registry lookup
/// (401) -> scope check (403). Records failed-auth attempts for the per-source
/// lockout and audits the outcome. Returns the matched client on success.
pub fn authenticate(
    state: &ApiState,
    headers: &HeaderMap,
    ip: IpAddr,
    scope: &str,
) -> Result<ApiClient, ApiError> {
    // 429 if this source is locked out (checked BEFORE any token work).
    state.lockout.check(ip)?;

    let token = match bearer_token(headers) {
        Some(t) => t,
        None => {
            state.lockout.record_failure(ip);
            crate::api::audit::record("-", "-", scope, "missing_token");
            return Err(ApiError::Unauthorized(
                "missing or malformed Authorization: Bearer header".to_string(),
            ));
        }
    };

    let client = match state.store.authenticate(&token) {
        Some(c) => c,
        None => {
            state.lockout.record_failure(ip);
            crate::api::audit::record("-", "-", scope, "invalid_token");
            return Err(ApiError::Unauthorized(
                "invalid, revoked, or expired token".to_string(),
            ));
        }
    };

    // A valid token proves this source is not a brute-forcer: clear its history.
    state.lockout.record_success(ip);

    if !client.has_scope(scope) {
        crate::api::audit::record(&client.client_id, &client.bound_fqn, scope, "forbidden_scope");
        return Err(ApiError::Forbidden(format!(
            "token is not scoped for '{}'",
            scope
        )));
    }

    crate::api::audit::record(&client.client_id, &client.bound_fqn, scope, "authenticated");
    Ok(client)
}
