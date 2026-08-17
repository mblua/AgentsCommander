//! Dedicated exact PTY-input POST and metadata-only GET routes.

use std::net::{IpAddr, SocketAddr};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, OriginalUri, Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tauri::Manager;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::message_store::{MessageStoreError, PtyInputEnqueueRequest};
use crate::api::schema::{PtyInputRequest, API_VERSION};
use crate::api::{handlers, ApiState};
use crate::phone::types::{
    canonical_pty_timestamp, pty_input_request_fingerprint, sha256_hex, PtyInputEnterMode,
    PtyInputSourcePlane, PTY_INPUT_HOST_ENVELOPE_MAX_BYTES, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
};

pub async fn post(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match post_inner(&state, addr.ip(), &uri, &headers, &body).await {
        Ok((status, result)) => (status, Json(result)).into_response(),
        Err(error) => {
            crate::api::audit::record_pty_input(&crate::api::audit::PtyInputAuditMetadata {
                event: "ingress_rejection".to_string(),
                injection_id: None,
                op_id: None,
                sender_fqn: None,
                target_fqn: None,
                payload_bytes: None,
                payload_sha256: None,
                source_plane: None,
                selected_session_id: None,
                selected_backend: None,
                status: "rejected".to_string(),
                reason_code: Some(error.code().to_string()),
                timestamp: canonical_pty_timestamp(chrono::Utc::now()),
            });
            error.into_response()
        }
    }
}

pub async fn get(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    AxumPath(op_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    match get_inner(&state, addr.ip(), &uri, &headers, &op_id).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => error.into_response(),
    }
}

fn exactly_one_header(
    headers: &HeaderMap,
    name: header::HeaderName,
) -> Result<Option<&str>, ApiError> {
    let values: Vec<_> = headers.get_all(name).iter().collect();
    match values.len() {
        0 => Ok(None),
        1 => values[0]
            .to_str()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("invalid_header".to_string())),
        _ => Err(ApiError::BadRequest("duplicate_header".to_string())),
    }
}

fn validate_content_headers(headers: &HeaderMap) -> Result<(), ApiError> {
    let content_type = exactly_one_header(headers, header::CONTENT_TYPE)?
        .ok_or_else(|| ApiError::BadRequest("content_type_required".to_string()))?;
    let mut parts = content_type.split(';');
    let media_type = parts.next().unwrap_or("").trim();
    let parameter = parts.next().map(str::trim);
    if !media_type.eq_ignore_ascii_case("application/json")
        || parts.next().is_some()
        || parameter.is_some_and(|value| !value.eq_ignore_ascii_case("charset=utf-8"))
    {
        return Err(ApiError::BadRequest("invalid_content_type".to_string()));
    }
    if let Some(encoding) = exactly_one_header(headers, header::CONTENT_ENCODING)? {
        if !encoding.trim().eq_ignore_ascii_case("identity") {
            return Err(ApiError::BadRequest("invalid_content_encoding".to_string()));
        }
    }
    Ok(())
}

async fn post_inner(
    state: &ApiState,
    ip: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(StatusCode, crate::phone::types::PtyInputResult), ApiError> {
    if body.len() > PTY_INPUT_HOST_ENVELOPE_MAX_BYTES {
        return Err(pty_input_error(
            crate::phone::types::PtyInputReasonCode::PayloadTooLarge,
        ));
    }
    let fresh = handlers::authenticate_pty_input_fresh(state, headers, ip).await?;
    if uri.query().is_some() {
        return Err(ApiError::BadRequest("query_not_allowed".to_string()));
    }
    validate_content_headers(headers)?;
    let value = crate::path_identity::parse_json_no_duplicates(body)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidEnvelope))?;
    let request: PtyInputRequest = serde_json::from_value(value)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidEnvelope))?;
    if request.api_version != API_VERSION || request.pty_input.version != PTY_INPUT_VERSION {
        return Err(pty_input_error(
            crate::phone::types::PtyInputReasonCode::UnsupportedVersion,
        ));
    }
    if request.pty_input.enter != PtyInputEnterMode::AgentSubmit {
        return Err(pty_input_error(
            crate::phone::types::PtyInputReasonCode::InvalidEnterMode,
        ));
    }
    crate::phone::types::parse_canonical_uuid_v4(&request.op_id)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidId))?;
    crate::config::teams::validate_pty_input_target_syntax(&request.to)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidTarget))?;
    if let Err(error) = crate::pty::inject::validate_pty_input_text(&request.pty_input.text) {
        return Err(match error.kind {
            crate::pty::inject::PtyInputTextErrorKind::TooLarge => {
                pty_input_error(crate::phone::types::PtyInputReasonCode::PayloadTooLarge)
            }
            _ => pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidText),
        });
    }
    let agent_id = match request.agent_id.as_deref() {
        None | Some("auto") => None,
        Some(agent_id) => {
            crate::config::coding_agent_mutations::validate_custom_agent_id(agent_id).map_err(
                |_| pty_input_error(crate::phone::types::PtyInputReasonCode::UnsupportedProfile),
            )?;
            Some(agent_id.to_string())
        }
    };
    let proof = crate::api::identity::InitialApiCredentialProof::from_fresh_guard(fresh)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::ApiClientUnbound))?;
    let authority = crate::api::identity::verify_live_pty_input_authority(state, proof).await?;
    let in_memory_project_paths = {
        let settings = state
            .app_handle
            .state::<crate::config::settings::SettingsState>();
        let paths = settings.read().await.project_paths.clone();
        paths
    };
    let mut project_paths =
        crate::config::settings::read_pty_input_project_paths_strict_offloaded()
            .await
            .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::UnsafePath))?
            .unwrap_or(in_memory_project_paths);
    if let Some(project_path) = authority.sender.ac_root_identity.canonical_path.parent() {
        let project = project_path
            .to_str()
            .map(crate::path_utils::normalize_windows_verbatim_path)
            .ok_or_else(|| pty_input_error(crate::phone::types::PtyInputReasonCode::UnsafePath))?;
        if !project_paths.contains(&project) {
            project_paths.push(project);
        }
    }
    let route = crate::config::teams::verify_pty_input_route(
        &authority.sender.replica_root,
        false,
        &request.to,
        &project_paths,
    )
    .map_err(|code| pty_input_error(route_reason_code(&code)))?;
    if route.sender.authority_fingerprint != authority.sender.authority_fingerprint {
        return Err(pty_input_error(
            crate::phone::types::PtyInputReasonCode::AuthorityChanged,
        ));
    }

    let injection_id = Uuid::new_v4().to_string();
    let nonce = Uuid::new_v4().to_string();
    let nonce_sha256 = sha256_hex(nonce.as_bytes());
    let issued = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(issued);
    let expires_at =
        canonical_pty_timestamp(issued + chrono::Duration::seconds(PTY_INPUT_TTL_SECS));
    let payload_sha256 = sha256_hex(request.pty_input.text.as_bytes());
    let normalized_agent = agent_id.as_deref().unwrap_or("");
    let fingerprint = pty_input_request_fingerprint(&[
        b"container_api",
        route.sender.canonical_fqn.as_bytes(),
        route.target.canonical_fqn.as_bytes(),
        b"1",
        b"agent-submit",
        payload_sha256.as_bytes(),
        &(request.pty_input.text.len() as u64).to_be_bytes(),
        normalized_agent.as_bytes(),
    ]);
    let result = state
        .message_store
        .enqueue_pty_input_offloaded(PtyInputEnqueueRequest {
            injection_id,
            sender_fqn: route.sender.canonical_fqn,
            target_fqn: route.target.canonical_fqn,
            op_id: request.op_id,
            nonce_sha256,
            request_fingerprint: fingerprint,
            confirmation_tag: None,
            requested_agent_id: agent_id,
            payload: request.pty_input.text.into_bytes(),
            source_plane: PtyInputSourcePlane::ContainerApi,
            sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
            sender_identity_fingerprint: route.sender.authority_fingerprint,
            target_identity_fingerprint: route.target.authority_fingerprint,
            authority_session_id: authority.session_id.to_string(),
            authority_client_id: Some(authority.client_id),
            authority_client_generation: Some(authority.credential_generation),
            issued_at,
            expires_at,
        })
        .await
        .map_err(map_store_error)?;
    let status = if result.duplicate && result.result.terminal {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    record_result_audit(
        if result.duplicate {
            "duplicate"
        } else {
            "enqueue"
        },
        &result.result,
    );
    Ok((status, result.result))
}

async fn get_inner(
    state: &ApiState,
    ip: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
    op_id: &str,
) -> Result<crate::phone::types::PtyInputResult, ApiError> {
    let fresh = handlers::authenticate_pty_input_fresh(state, headers, ip).await?;
    if uri.query().is_some() {
        return Err(ApiError::BadRequest("query_not_allowed".to_string()));
    }
    crate::phone::types::parse_canonical_uuid_v4(op_id)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::InvalidId))?;
    let proof = crate::api::identity::InitialApiCredentialProof::from_fresh_guard(fresh)
        .map_err(|_| pty_input_error(crate::phone::types::PtyInputReasonCode::ApiClientUnbound))?;
    let authority = crate::api::identity::verify_live_pty_input_authority(state, proof).await?;
    state
        .message_store
        .query_pty_input_offloaded(
            authority.sender.canonical_fqn,
            op_id.to_string(),
            authority.sender.incarnation_fingerprint,
        )
        .await
        .map_err(map_store_error)?
        .ok_or_else(|| ApiError::NotFound("operation_not_found".to_string()))
}

fn record_result_audit(event: &str, result: &crate::phone::types::PtyInputResult) {
    crate::api::audit::record_pty_input_result(event, result);
}

fn pty_input_error(code: crate::phone::types::PtyInputReasonCode) -> ApiError {
    ApiError::PtyInput(crate::phone::types::PtyInputFailure::reject(code))
}

fn route_reason_code(code: &str) -> crate::phone::types::PtyInputReasonCode {
    use crate::phone::types::PtyInputReasonCode as C;
    match code {
        "invalid_target" => C::InvalidTarget,
        "sender_identity_invalid" => C::SenderIdentityInvalid,
        "sender_not_coordinator" => C::SenderNotCoordinator,
        "root_identity_invalid" => C::RootIdentityInvalid,
        "target_not_member" => C::TargetNotMember,
        "target_is_coordinator" => C::TargetIsCoordinator,
        "target_out_of_scope" => C::TargetOutOfScope,
        "unsafe_path" => C::UnsafePath,
        _ => C::AuthorityChanged,
    }
}

fn map_store_error(error: MessageStoreError) -> ApiError {
    match error {
        MessageStoreError::IdempotencyConflict => {
            pty_input_error(crate::phone::types::PtyInputReasonCode::IdempotencyConflict)
        }
        MessageStoreError::CapacityExceeded => {
            pty_input_error(crate::phone::types::PtyInputReasonCode::CapacityExceeded)
        }
        MessageStoreError::OperationNotFound => {
            ApiError::NotFound("operation_not_found".to_string())
        }
        MessageStoreError::StoreCorrupt
        | MessageStoreError::UnsafePath
        | MessageStoreError::InvalidTransition
        | MessageStoreError::ActuationCommitAmbiguous => {
            pty_input_error(crate::phone::types::PtyInputReasonCode::StoreCorrupt)
        }
        _ => pty_input_error(crate::phone::types::PtyInputReasonCode::StoreTransient),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_request_rejects_caller_selected_authority_fields() {
        for field in [
            "from",
            "token",
            "root",
            "sourcePlane",
            "sessionId",
            "backend",
            "command",
        ] {
            let body = format!(
                r#"{{"apiVersion":"1","opId":"{}","to":"p:wg-1-t/a","ptyInput":{{"version":1,"text":"x","enter":"agent-submit"}},"{field}":"x"}}"#,
                Uuid::new_v4()
            );
            let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
            assert!(
                serde_json::from_value::<PtyInputRequest>(value).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn unsupported_enter_value_remains_distinguishable_from_a_malformed_envelope() {
        let body = format!(
            r#"{{"apiVersion":"1","opId":"{}","to":"p:wg-1-t/a","ptyInput":{{"version":1,"text":"x","enter":"other"}}}}"#,
            Uuid::new_v4()
        );
        let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
        let request: PtyInputRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.pty_input.enter, PtyInputEnterMode::Unsupported);
    }

    #[test]
    fn duplicate_nested_key_is_rejected() {
        let body = format!(
            r#"{{"apiVersion":"1","opId":"{}","to":"p:wg-1-t/a","ptyInput":{{"version":1,"text":"x","text":"y","enter":"agent-submit"}}}}"#,
            Uuid::new_v4()
        );
        assert!(crate::path_identity::parse_json_no_duplicates(body.as_bytes()).is_err());
    }

    #[test]
    fn content_headers_reject_duplicates_parameters_and_encodings() {
        let mut valid = HeaderMap::new();
        valid.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(validate_content_headers(&valid).is_ok());

        let mut utf8 = HeaderMap::new();
        utf8.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        utf8.insert(header::CONTENT_ENCODING, "identity".parse().unwrap());
        assert!(validate_content_headers(&utf8).is_ok());

        for invalid in [
            "application/json; charset=latin1",
            "application/json; profile=x",
            "text/json",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, invalid.parse().unwrap());
            assert!(validate_content_headers(&headers).is_err(), "{invalid}");
        }

        let mut duplicate = HeaderMap::new();
        duplicate.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
        duplicate.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(validate_content_headers(&duplicate).is_err());

        let mut encoded = HeaderMap::new();
        encoded.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        encoded.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        assert!(validate_content_headers(&encoded).is_err());
    }

    #[test]
    fn escaped_maximum_text_fits_raw_envelope_but_decoded_overage_is_rejected() {
        let op_id = Uuid::new_v4();
        let escaped = "\\u0078".repeat(crate::pty::backend::PTY_INPUT_MAX_BYTES);
        let body = format!(
            r#"{{"apiVersion":"1","opId":"{op_id}","to":"p:wg-1-t/a","ptyInput":{{"version":1,"text":"{escaped}","enter":"agent-submit"}}}}"#
        );
        assert!(body.len() <= PTY_INPUT_HOST_ENVELOPE_MAX_BYTES);
        let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
        let request: PtyInputRequest = serde_json::from_value(value).unwrap();
        assert_eq!(
            request.pty_input.text.len(),
            crate::pty::backend::PTY_INPUT_MAX_BYTES
        );
        assert!(crate::pty::inject::validate_pty_input_text(&request.pty_input.text).is_ok());

        let over = format!("{}x", request.pty_input.text);
        assert_eq!(over.len(), crate::pty::backend::PTY_INPUT_MAX_BYTES + 1);
        assert!(crate::pty::inject::validate_pty_input_text(&over).is_err());
    }
}
