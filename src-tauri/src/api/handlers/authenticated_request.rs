//! Shared, handler-local credential admission. This module deliberately does
//! not read request bodies, establish deadlines, construct responses, or own
//! operation audit finalization.

use std::net::IpAddr;

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use crate::api::auth::FreshRegistryError;
use crate::api::identity::{
    BoundContainerCoordinatorError, InitialApiCredentialProof, VerifiedBoundContainerCoordinator,
};
use crate::api::window_target_registry::CallerBinding;
use crate::api::ApiState;

pub(super) struct InitialAuthenticatedRequest {
    authority: VerifiedBoundContainerCoordinator,
}

impl InitialAuthenticatedRequest {
    pub(super) fn into_authority(self) -> VerifiedBoundContainerCoordinator {
        self.authority
    }

    pub(super) fn caller_binding(&self, salt: &[u8; 32]) -> CallerBinding {
        let mut hasher = Sha256::new();
        hasher.update(salt);
        update_length_framed(&mut hasher, self.authority.client_id.as_bytes());
        let root_object_id = format!("{:?}", self.authority.bound_root_object_id);
        update_length_framed(&mut hasher, root_object_id.as_bytes());
        CallerBinding::from_digest(hasher.finalize().into())
    }
}

fn update_length_framed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

pub(super) enum AuthenticatedRequestError {
    RateLimited,
    AuthenticationFailed,
    ServiceUnavailable,
    ScopeDenied,
    BoundAuthority(BoundContainerCoordinatorError),
}

/// Performs only the credential and live-bound-authority admission that was
/// previously embedded in terminal snapshot. Callers retain their operation's
/// ingress, body deadline, response, audit, and final authority behavior.
pub(super) async fn pre_admit(
    state: &ApiState,
    address: IpAddr,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<InitialAuthenticatedRequest, AuthenticatedRequestError> {
    state
        .lockout
        .check(address)
        .map_err(|_| AuthenticatedRequestError::RateLimited)?;
    let bearer = match super::bearer_token_strict(headers) {
        Ok(token) => token,
        Err(_) => {
            state
                .lockout
                .record_failure(address)
                .map_err(|_| AuthenticatedRequestError::ServiceUnavailable)?;
            return Err(AuthenticatedRequestError::AuthenticationFailed);
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
                .record_failure(address)
                .map_err(|_| AuthenticatedRequestError::ServiceUnavailable)?;
            return Err(AuthenticatedRequestError::AuthenticationFailed);
        }
        Err(FreshRegistryError::Contended | FreshRegistryError::Internal) => {
            return Err(AuthenticatedRequestError::ServiceUnavailable)
        }
    };
    state
        .lockout
        .record_success(address)
        .map_err(|_| AuthenticatedRequestError::ServiceUnavailable)?;
    if !fresh.client.has_scope(required_scope)
        || fresh.client.bound_session_id.is_none()
        || fresh.client.credential_generation.is_none()
    {
        return Err(AuthenticatedRequestError::ScopeDenied);
    }
    let proof = InitialApiCredentialProof::from_fresh_guard(fresh)
        .map_err(AuthenticatedRequestError::BoundAuthority)?;
    let authority = crate::api::identity::verify_live_bound_container_coordinator(state, proof)
        .await
        .map_err(AuthenticatedRequestError::BoundAuthority)?;

    Ok(InitialAuthenticatedRequest { authority })
}
