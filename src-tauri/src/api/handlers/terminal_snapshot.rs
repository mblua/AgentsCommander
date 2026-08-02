use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, OriginalUri, State};
use axum::http::{header, HeaderMap, HeaderName, Request, StatusCode};
use axum::response::Response;
use tauri::Manager;
use terminal_snapshot_renderer::{
    decode_bounded, to_ascii_json, TerminalSnapshotReasonCode, MAX_REQUEST_BYTES,
};

use crate::api::auth::{FreshRegistryError, SCOPE_TERMINAL_SNAPSHOT};
use crate::api::identity::{BoundContainerCoordinatorError, InitialApiCredentialProof};
use crate::api::schema::{TerminalSnapshotApiError, TerminalSnapshotApiRequest};
use crate::api::ApiState;
use crate::pty::terminal_snapshot::{
    TerminalSnapshotAuditGuard, TerminalSnapshotRequesterSelector, TerminalSnapshotServiceContext,
    TerminalSnapshotServiceRequest, TerminalSnapshotSourcePlane, TerminalSnapshotState,
};

pub async fn post(
    State(state): State<ApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response {
    let audit =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let response = post_inner(state, address, uri, request, audit.clone()).await;
    match response {
        Ok(response) => response,
        Err(reason) => {
            audit.finalize_failure(reason);
            error_response(reason)
        }
    }
}

async fn post_inner(
    state: ApiState,
    address: SocketAddr,
    uri: axum::http::Uri,
    request: Request<Body>,
    audit: TerminalSnapshotAuditGuard,
) -> Result<Response, TerminalSnapshotReasonCode> {
    let snapshot_state = state
        .app_handle
        .try_state::<Arc<TerminalSnapshotState>>()
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?;
    let ingress = snapshot_state.try_admit_ingress(format!("api:{}", address.ip()))?;
    if uri.query().is_some() {
        return Err(TerminalSnapshotReasonCode::InvalidRequest);
    }
    let (parts, body) = request.into_parts();
    validate_request_headers(&parts.headers)?;
    state
        .lockout
        .check(address.ip())
        .map_err(|_| TerminalSnapshotReasonCode::RateLimited)?;
    let bearer = match crate::api::handlers::bearer_token_strict(&parts.headers) {
        Ok(token) => token,
        Err(_) => {
            state
                .lockout
                .record_failure(address.ip())
                .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
            return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
        }
    };
    let fresh = match state
        .store
        .authenticate_privileged_fresh_offloaded(bearer)
        .await
    {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            state
                .lockout
                .record_failure(address.ip())
                .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
            return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
        }
        Err(FreshRegistryError::Contended | FreshRegistryError::Internal) => {
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable)
        }
    };
    state
        .lockout
        .record_success(address.ip())
        .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
    if !fresh.client.has_scope(SCOPE_TERMINAL_SNAPSHOT)
        || fresh.client.bound_session_id.is_none()
        || fresh.client.credential_generation.is_none()
    {
        return Err(TerminalSnapshotReasonCode::NotAuthorized);
    }
    let proof =
        InitialApiCredentialProof::from_fresh_guard(fresh).map_err(map_bound_authority_error)?;
    let authority = crate::api::identity::verify_live_bound_container_coordinator(&state, proof)
        .await
        .map_err(map_bound_authority_error)?;

    let settings = state
        .app_handle
        .try_state::<crate::config::settings::SettingsState>()
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?
        .inner()
        .clone();
    let restore = state
        .app_handle
        .try_state::<Arc<crate::RestoreInProgress>>()
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?
        .inner()
        .clone();
    let purge = state
        .app_handle
        .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?
        .inner()
        .clone();
    let context = TerminalSnapshotServiceContext {
        session_manager: Arc::clone(&state.session_mgr),
        pty_manager: Arc::clone(&state.pty_mgr),
        settings,
        restore,
        purge,
    };
    let admission = snapshot_state
        .pre_admit_requester(
            &context,
            TerminalSnapshotRequesterSelector::ApiSession(authority.session_id),
            TerminalSnapshotSourcePlane::ContainerApi,
            None,
            audit.clone(),
        )
        .await?;
    let deadline = admission.deadline();
    drop(ingress);

    let claimed_length = parse_content_length(&parts.headers)?;
    let bytes = collect_request_body(body, deadline).await?;
    if claimed_length.is_some_and(|length| length != bytes.len()) {
        return Err(TerminalSnapshotReasonCode::InvalidRequest);
    }
    let request: TerminalSnapshotApiRequest = decode_bounded(&bytes, MAX_REQUEST_BYTES)
        .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
    request
        .validate()
        .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
    let service_request = TerminalSnapshotServiceRequest {
        request_id: terminal_snapshot_renderer::validate_uuid(&request.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?,
        target: request.to,
        format: request.format,
        source_plane: TerminalSnapshotSourcePlane::ContainerApi,
        host_authorization_deadline: None,
    };
    let prepared = snapshot_state
        .prepare_with_admission(admission, service_request)
        .await?;
    let (payload, finalization) = prepared.into_parts();
    let response_bytes = finalization.build_api_response(payload).await?;
    let disclosure = finalization.revalidate_api().await?;
    #[cfg(test)]
    snapshot_state.run_api_final_handoff_hook();

    // Every source-plane byte and awaited authority check is complete. The
    // fresh registry guard, synchronous runtime proof, immutable response
    // construction, and audit finalization are the sole final handoff.
    let remaining = disclosure.remaining()?;
    let final_guard = match tokio::time::timeout(
        remaining,
        state.store.load_active_binding_fresh_offloaded(
            authority.client_id.clone(),
            authority.credential_generation.clone(),
            SCOPE_TERMINAL_SNAPSHOT,
        ),
    )
    .await
    {
        Err(_) => {
            disclosure.finalize_failure(TerminalSnapshotReasonCode::SnapshotTimeout);
            return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
        }
        Ok(Ok(Some(guard))) => guard,
        Ok(Ok(None)) => {
            disclosure.finalize_failure(TerminalSnapshotReasonCode::AuthorityChanged);
            return Err(TerminalSnapshotReasonCode::AuthorityChanged);
        }
        Ok(Err(_)) => {
            disclosure.finalize_failure(TerminalSnapshotReasonCode::ServiceUnavailable);
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
    };
    if let Err(reason) = disclosure.ensure_handoff() {
        let response = error_response(reason);
        drop(final_guard);
        disclosure.finalize_failure(reason);
        return Ok(response);
    }
    let authorized = crate::api::identity::verify_final_bound_container_coordinator(
        &state,
        &authority,
        &final_guard,
    );
    let response = if authorized {
        json_response(StatusCode::OK, response_bytes)
    } else {
        error_response(TerminalSnapshotReasonCode::AuthorityChanged)
    };
    drop(final_guard);
    if authorized {
        disclosure.finalize_success();
    } else {
        disclosure.finalize_failure(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    Ok(response)
}

async fn collect_request_body(
    body: Body,
    deadline: std::time::Instant,
) -> Result<axum::body::Bytes, TerminalSnapshotReasonCode> {
    let remaining = deadline
        .checked_duration_since(std::time::Instant::now())
        .ok_or(TerminalSnapshotReasonCode::SnapshotTimeout)?;
    tokio::time::timeout(remaining, to_bytes(body, MAX_REQUEST_BYTES))
        .await
        .map_err(|_| TerminalSnapshotReasonCode::SnapshotTimeout)?
        .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)
}

fn map_bound_authority_error(error: BoundContainerCoordinatorError) -> TerminalSnapshotReasonCode {
    match error {
        BoundContainerCoordinatorError::Unbound
        | BoundContainerCoordinatorError::NotCoordinator => {
            TerminalSnapshotReasonCode::NotAuthorized
        }
        BoundContainerCoordinatorError::Stale | BoundContainerCoordinatorError::BindingMismatch => {
            TerminalSnapshotReasonCode::RequesterUnavailable
        }
        BoundContainerCoordinatorError::Internal => TerminalSnapshotReasonCode::ServiceUnavailable,
    }
}

fn exactly_one_header(
    headers: &HeaderMap,
    name: HeaderName,
) -> Result<Option<&str>, TerminalSnapshotReasonCode> {
    let values: Vec<_> = headers.get_all(name).iter().collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => value
            .to_str()
            .map(Some)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest),
        _ => Err(TerminalSnapshotReasonCode::InvalidRequest),
    }
}

fn validate_request_headers(headers: &HeaderMap) -> Result<(), TerminalSnapshotReasonCode> {
    exactly_one_header(headers, header::AUTHORIZATION)?
        .ok_or(TerminalSnapshotReasonCode::RequesterUnavailable)?;
    let content_type = exactly_one_header(headers, header::CONTENT_TYPE)?
        .ok_or(TerminalSnapshotReasonCode::InvalidRequest)?;
    let mut content_type_parts = content_type.split(';');
    let media_type = content_type_parts.next().unwrap_or("").trim();
    let parameter = content_type_parts.next().map(str::trim);
    if !media_type.eq_ignore_ascii_case("application/json")
        || content_type_parts.next().is_some()
        || parameter.is_some_and(|value| !value.eq_ignore_ascii_case("charset=utf-8"))
    {
        return Err(TerminalSnapshotReasonCode::InvalidRequest);
    }
    if let Some(encoding) = exactly_one_header(headers, header::CONTENT_ENCODING)? {
        if !encoding.eq_ignore_ascii_case("identity") {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
    }
    exactly_one_header(headers, header::CONTENT_LENGTH)?;
    Ok(())
}

fn parse_content_length(headers: &HeaderMap) -> Result<Option<usize>, TerminalSnapshotReasonCode> {
    exactly_one_header(headers, header::CONTENT_LENGTH)?
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|length| *length <= MAX_REQUEST_BYTES)
                .ok_or(TerminalSnapshotReasonCode::InvalidRequest)
        })
        .transpose()
}

fn error_response(reason: TerminalSnapshotReasonCode) -> Response {
    let status = reason
        .http_status()
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let envelope = TerminalSnapshotApiError::new(reason);
    let bytes = to_ascii_json(&envelope, terminal_snapshot_renderer::MAX_ERROR_BYTES)
        .unwrap_or_else(|_| {
            b"{\"apiVersion\":\"1\",\"error\":\"internal\",\"detail\":\"An internal terminal snapshot invariant failed.\"}".to_vec()
        });
    json_response(status, bytes)
}

fn json_response(status: StatusCode, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_strict_and_no_store_is_unconditional() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(validate_request_headers(&headers).is_ok());
        headers.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(validate_request_headers(&headers).is_err());

        let response = error_response(TerminalSnapshotReasonCode::InvalidRequest);
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::PRAGMA], "no-cache");
    }

    #[tokio::test]
    async fn authenticated_body_collection_obeys_the_server_deadline() {
        let stream = futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>();
        let body = Body::from_stream(stream);
        let result = collect_request_body(
            body,
            std::time::Instant::now() + std::time::Duration::from_millis(20),
        )
        .await;
        assert!(matches!(
            result,
            Err(TerminalSnapshotReasonCode::SnapshotTimeout)
        ));
    }

    #[test]
    fn fixed_status_reason_mapping_is_preserved() {
        for reason in [
            TerminalSnapshotReasonCode::InvalidRequest,
            TerminalSnapshotReasonCode::RequesterUnavailable,
            TerminalSnapshotReasonCode::TerminalSnapshotsDisabled,
            TerminalSnapshotReasonCode::NotAuthorized,
            TerminalSnapshotReasonCode::TargetUnavailable,
            TerminalSnapshotReasonCode::SnapshotUnavailable,
            TerminalSnapshotReasonCode::SnapshotTooLarge,
            TerminalSnapshotReasonCode::AuthorityChanged,
            TerminalSnapshotReasonCode::RateLimited,
            TerminalSnapshotReasonCode::SnapshotTimeout,
            TerminalSnapshotReasonCode::ServiceUnavailable,
            TerminalSnapshotReasonCode::RenderFailed,
            TerminalSnapshotReasonCode::UnsafePath,
            TerminalSnapshotReasonCode::ResponseUnavailable,
            TerminalSnapshotReasonCode::Internal,
        ] {
            let response = error_response(reason);
            assert_eq!(
                response.status().as_u16(),
                reason.http_status().expect("server reason has status")
            );
        }
    }
}
