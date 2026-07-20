//! HTTP handlers for the control-plane API (#791 §6, §7).

pub mod list_peers;
pub mod pty_input;
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
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = raw.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub fn bearer_token_strict(headers: &HeaderMap) -> Result<String, ApiError> {
    let values: Vec<_> = headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .collect();
    if values.len() != 1 {
        return Err(ApiError::Unauthorized(
            "missing_or_duplicate_authorization".to_string(),
        ));
    }
    let raw = values[0]
        .to_str()
        .map_err(|_| ApiError::Unauthorized("malformed_authorization".to_string()))?;
    let token = raw
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("malformed_authorization".to_string()))?;
    Ok(token.to_string())
}

pub async fn authenticate_pty_input_fresh(
    state: &ApiState,
    headers: &HeaderMap,
    ip: IpAddr,
) -> Result<crate::api::auth::ApiClientFreshGuard, ApiError> {
    state.lockout.check(ip)?;
    let token = match bearer_token_strict(headers) {
        Ok(token) => token,
        Err(error) => {
            state.lockout.record_failure(ip)?;
            return Err(error);
        }
    };
    let guard = match state
        .store
        .authenticate_pty_input_fresh_offloaded(token)
        .await
    {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            state.lockout.record_failure(ip)?;
            return Err(ApiError::PtyInput(
                crate::phone::types::PtyInputFailure::reject(
                    crate::phone::types::PtyInputReasonCode::ApiClientStale,
                ),
            ));
        }
        Err(
            crate::api::auth::FreshRegistryError::Contended
            | crate::api::auth::FreshRegistryError::Internal,
        ) => {
            return Err(ApiError::PtyInput(
                crate::phone::types::PtyInputFailure::retry(
                    crate::phone::types::PtyInputReasonCode::StoreTransient,
                ),
            ));
        }
    };
    state.lockout.record_success(ip)?;
    if !guard.client.has_scope(crate::api::auth::SCOPE_PTY_INPUT) {
        return Err(ApiError::PtyInput(
            crate::phone::types::PtyInputFailure::reject(
                crate::phone::types::PtyInputReasonCode::ApiScopeRequired,
            ),
        ));
    }
    if guard.client.bound_session_id.is_none() || guard.client.credential_generation.is_none() {
        return Err(ApiError::PtyInput(
            crate::phone::types::PtyInputFailure::reject(
                crate::phone::types::PtyInputReasonCode::ApiClientUnbound,
            ),
        ));
    }
    Ok(guard)
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
            state.lockout.record_failure(ip)?;
            crate::api::audit::record("-", "-", scope, "missing_token");
            return Err(ApiError::Unauthorized(
                "missing or malformed Authorization: Bearer header".to_string(),
            ));
        }
    };

    let client = match state.store.authenticate(&token) {
        Ok(Some(c)) => c,
        Ok(None) => {
            state.lockout.record_failure(ip)?;
            crate::api::audit::record("-", "-", scope, "invalid_token");
            return Err(ApiError::Unauthorized(
                "invalid, revoked, or expired token".to_string(),
            ));
        }
        Err(e) => {
            crate::api::audit::record("-", "-", scope, "auth_internal_error");
            return Err(e);
        }
    };

    // A valid token proves this source is not a brute-forcer: clear its history.
    state.lockout.record_success(ip)?;

    if !client.has_scope(scope) {
        crate::api::audit::record(
            &client.client_id,
            &client.bound_fqn,
            scope,
            "forbidden_scope",
        );
        return Err(ApiError::Forbidden(format!(
            "token is not scoped for '{}'",
            scope
        )));
    }

    crate::api::audit::record(&client.client_id, &client.bound_fqn, scope, "authenticated");
    Ok(client)
}
