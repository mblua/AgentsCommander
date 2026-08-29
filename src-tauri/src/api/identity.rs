//! API identity resolution (#791 §9, §0.5 G5 + HIGH-3).
//!
//! The API replaces the filesystem model's LOCATION-based identity with a
//! TOKEN-based one. The `from` is derived AT REQUEST TIME from the matched
//! client's `boundRoot` via `agent_fqn_from_path` (the SAME function the CLI and
//! daemon use), never from client input and never from the stored `boundFqn`
//! hint (which can go stale if the replica is relocated).

use super::auth::ApiClient;
use super::error::ApiError;

/// Resolve the sender FQN for an authenticated client.
///
/// - 401 if `boundRoot` no longer exists (the replica was removed; re-mint).
/// - 403 if the derived FQN is the Root Agent (root-agent is API-excluded in
///   increment 1, §0.5 HIGH-3). This is defense-in-depth behind the mint-time
///   guard.
/// - Logs a `WARN` if the freshly-derived FQN differs from the stored
///   `boundFqn` hint (replica moved since mint), and uses the fresh value.
pub fn resolve_from(client: &ApiClient) -> Result<String, ApiError> {
    if !std::path::Path::new(&client.bound_root).exists() {
        return Err(ApiError::Unauthorized(
            "bound replica no longer exists; re-mint".to_string(),
        ));
    }

    let fqn = crate::config::teams::agent_fqn_from_path(&client.bound_root);

    if crate::config::root_agent::is_root_agent_target(&fqn)
        || fqn == crate::config::root_agent::ROOT_AGENT_SENDER
    {
        return Err(ApiError::Forbidden(
            "root-agent not reachable over the API in increment 1".to_string(),
        ));
    }

    if fqn != client.bound_fqn {
        log::warn!(
            "[api-identity] client {} replica moved since mint (stored '{}' != derived '{}'); re-mint recommended",
            client.client_id,
            client.bound_fqn,
            fqn
        );
    }

    Ok(fqn)
}

pub struct InitialApiCredentialProof {
    pub client_id: String,
    pub bound_root: String,
    pub bound_session_id: String,
    pub credential_generation: String,
    pub presented_token_hash: String,
}

impl InitialApiCredentialProof {
    pub fn from_fresh_guard(
        guard: crate::api::auth::ApiClientFreshGuard,
    ) -> Result<Self, BoundContainerCoordinatorError> {
        let client = guard.client;
        let bound_session_id = client
            .bound_session_id
            .ok_or(BoundContainerCoordinatorError::Unbound)?;
        let credential_generation = client
            .credential_generation
            .ok_or(BoundContainerCoordinatorError::Unbound)?;
        Ok(Self {
            client_id: client.client_id,
            bound_root: client.bound_root,
            bound_session_id,
            credential_generation,
            presented_token_hash: guard.presented_token_hash,
        })
    }
}

impl std::fmt::Debug for InitialApiCredentialProof {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InitialApiCredentialProof")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundContainerCoordinatorError {
    Unbound,
    Stale,
    BindingMismatch,
    NotCoordinator,
    Internal,
}

pub struct VerifiedBoundContainerCoordinator {
    pub sender: crate::config::teams::VerifiedPtyInputIdentity,
    pub session_id: uuid::Uuid,
    pub client_id: String,
    pub credential_generation: String,
    pub bound_root_object_id: crate::path_identity::FileObjectId,
    pub presented_token_hash: String,
}

impl std::fmt::Debug for VerifiedBoundContainerCoordinator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedBoundContainerCoordinator")
            .finish_non_exhaustive()
    }
}

pub async fn verify_live_bound_container_coordinator(
    state: &crate::api::ApiState,
    proof: InitialApiCredentialProof,
) -> Result<VerifiedBoundContainerCoordinator, BoundContainerCoordinatorError> {
    let session_id = crate::phone::types::parse_canonical_uuid_v4(&proof.bound_session_id)
        .map_err(|_| BoundContainerCoordinatorError::Unbound)?;
    crate::phone::types::parse_canonical_uuid_v4(&proof.credential_generation)
        .map_err(|_| BoundContainerCoordinatorError::Unbound)?;
    let manager = {
        let manager = state.session_mgr.read().await;
        manager.clone()
    };
    let fact = manager
        .live_snapshot_requester_by_id(session_id)
        .await
        .ok_or(BoundContainerCoordinatorError::Stale)?;
    if fact.backend_kind != crate::pty::backend::SessionBackendKind::ContainerTransport {
        return Err(BoundContainerCoordinatorError::Stale);
    }
    if fact.is_root_agent || !fact.is_coordinator {
        return Err(BoundContainerCoordinatorError::NotCoordinator);
    }
    let bound_root =
        crate::path_identity::verify_directory(std::path::Path::new(&proof.bound_root))
            .map_err(|_| BoundContainerCoordinatorError::BindingMismatch)?;
    let session_root =
        crate::path_identity::verify_directory(std::path::Path::new(&fact.working_directory))
            .map_err(|_| BoundContainerCoordinatorError::BindingMismatch)?;
    if !crate::path_identity::same_object(&bound_root, &session_root) {
        return Err(BoundContainerCoordinatorError::BindingMismatch);
    }
    let route = crate::pty::manager::PtyManager::snapshot_route_proof(&state.pty_mgr, session_id)
        .map_err(|_| BoundContainerCoordinatorError::BindingMismatch)?;
    let container_backend = {
        let manager = state
            .pty_mgr
            .lock()
            .map_err(|_| BoundContainerCoordinatorError::Internal)?;
        manager.container_backend()
    };
    let binding = container_backend
        .credential_binding(session_id)
        .ok_or(BoundContainerCoordinatorError::BindingMismatch)?;
    if binding.client_id != proof.client_id
        || binding.credential_generation != proof.credential_generation
        || binding.bound_session_id != proof.bound_session_id
        || binding.bound_root_object_id != bound_root.object_id
        || !crate::api::auth::constant_time_eq(
            &binding.credential_token_hash,
            &proof.presented_token_hash,
        )
    {
        return Err(BoundContainerCoordinatorError::BindingMismatch);
    }
    let sender = crate::config::teams::verify_pty_input_coordinator_root(std::path::Path::new(
        &fact.working_directory,
    ))
    .map_err(|_| BoundContainerCoordinatorError::NotCoordinator)?;
    if route.backend_kind() != crate::pty::backend::SessionBackendKind::ContainerTransport
        || route.liveness() != crate::pty::context_scrape::ContextSessionLiveness::Live
        || !route.matches_requester_route(
            crate::pty::backend::SessionBackendKind::ContainerTransport,
            &session_root,
            Some(&sender.replica_identity),
        )
    {
        return Err(BoundContainerCoordinatorError::BindingMismatch);
    }
    Ok(VerifiedBoundContainerCoordinator {
        sender,
        session_id,
        client_id: proof.client_id,
        credential_generation: proof.credential_generation,
        bound_root_object_id: bound_root.object_id,
        presented_token_hash: proof.presented_token_hash,
    })
}

pub fn verify_final_bound_container_coordinator(
    state: &crate::api::ApiState,
    authority: &VerifiedBoundContainerCoordinator,
    fresh: &crate::api::auth::ApiClientFreshGuard,
) -> bool {
    let client = &fresh.client;
    if client.client_id != authority.client_id
        || client.bound_session_id.as_deref() != Some(authority.session_id.to_string().as_str())
        || client.credential_generation.as_deref() != Some(authority.credential_generation.as_str())
        || !client.has_scope(crate::api::auth::SCOPE_TERMINAL_SNAPSHOT)
        || !crate::api::auth::constant_time_eq(
            &fresh.presented_token_hash,
            &authority.presented_token_hash,
        )
    {
        return false;
    }
    let fresh_bound_root = match verify_fresh_bound_root(
        client,
        authority.bound_root_object_id,
        &authority.sender.replica_identity,
    ) {
        Some(identity) => identity,
        None => return false,
    };
    let route = match crate::pty::manager::PtyManager::snapshot_route_proof(
        &state.pty_mgr,
        authority.session_id,
    ) {
        Ok(route) => route,
        Err(_) => return false,
    };
    let backend = match state.pty_mgr.lock() {
        Ok(manager) => manager.container_backend(),
        Err(_) => return false,
    };
    let Some(binding) = backend.credential_binding(authority.session_id) else {
        return false;
    };
    binding.client_id == authority.client_id
        && binding.credential_generation == authority.credential_generation
        && binding.bound_session_id == authority.session_id.to_string()
        && binding.bound_root_object_id == authority.bound_root_object_id
        && crate::api::auth::constant_time_eq(
            &binding.credential_token_hash,
            &authority.presented_token_hash,
        )
        && route.backend_kind() == crate::pty::backend::SessionBackendKind::ContainerTransport
        && route.liveness() == crate::pty::context_scrape::ContextSessionLiveness::Live
        && route.matches_requester_route(
            crate::pty::backend::SessionBackendKind::ContainerTransport,
            &fresh_bound_root,
            Some(&authority.sender.replica_identity),
        )
}

fn verify_fresh_bound_root(
    client: &ApiClient,
    expected_object: crate::path_identity::FileObjectId,
    expected_replica: &crate::path_identity::VerifiedPathIdentity,
) -> Option<crate::path_identity::VerifiedPathIdentity> {
    let identity =
        crate::path_identity::verify_directory(std::path::Path::new(&client.bound_root)).ok()?;
    (identity.object_id == expected_object
        && crate::path_identity::same_object(&identity, expected_replica))
    .then_some(identity)
}

pub struct VerifiedApiPtyAuthority {
    pub sender: crate::config::teams::VerifiedPtyInputIdentity,
    pub session_id: uuid::Uuid,
    pub client_id: String,
    pub credential_generation: String,
}

fn pty_authority_error(code: crate::phone::types::PtyInputReasonCode) -> ApiError {
    ApiError::PtyInput(crate::phone::types::PtyInputFailure::reject(code))
}

pub async fn verify_live_pty_input_authority(
    state: &crate::api::ApiState,
    proof: InitialApiCredentialProof,
) -> Result<VerifiedApiPtyAuthority, ApiError> {
    let authority = verify_live_bound_container_coordinator(state, proof)
        .await
        .map_err(|error| {
            let code = match error {
                BoundContainerCoordinatorError::Unbound => {
                    crate::phone::types::PtyInputReasonCode::ApiClientUnbound
                }
                BoundContainerCoordinatorError::Stale => {
                    crate::phone::types::PtyInputReasonCode::ApiClientStale
                }
                BoundContainerCoordinatorError::NotCoordinator => {
                    crate::phone::types::PtyInputReasonCode::SenderNotCoordinator
                }
                BoundContainerCoordinatorError::BindingMismatch
                | BoundContainerCoordinatorError::Internal => {
                    crate::phone::types::PtyInputReasonCode::ApiBindingMismatch
                }
            };
            pty_authority_error(code)
        })?;
    Ok(VerifiedApiPtyAuthority {
        sender: authority.sender,
        session_id: authority.session_id,
        client_id: authority.client_id,
        credential_generation: authority.credential_generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::ApiClient;

    fn client_with_root(root: &str, bound_fqn: &str) -> ApiClient {
        ApiClient {
            client_id: "c1".into(),
            label: "l".into(),
            token_hash: "sha256:x".into(),
            bound_fqn: bound_fqn.into(),
            bound_root: root.into(),
            scopes: vec!["send".into()],
            issued_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
            revoked: false,
            bound_session_id: None,
            credential_generation: None,
        }
    }

    #[test]
    fn initial_snapshot_credential_debug_omits_auth_and_path_canaries() {
        const AUTH_CANARY: &str = "AUTH_1173_API_G2D6";
        const PATH_CANARY: &str = r"C:\PATH_1173_API_G2D6\replica";
        let proof = InitialApiCredentialProof {
            client_id: AUTH_CANARY.to_string(),
            bound_root: PATH_CANARY.to_string(),
            bound_session_id: AUTH_CANARY.to_string(),
            credential_generation: AUTH_CANARY.to_string(),
            presented_token_hash: AUTH_CANARY.to_string(),
        };
        let diagnostic = format!("{proof:?}");
        assert!(!diagnostic.contains(AUTH_CANARY));
        assert!(!diagnostic.contains(PATH_CANARY));
        assert_eq!(diagnostic, "InitialApiCredentialProof { .. }");
    }

    #[test]
    fn missing_bound_root_is_401() {
        let c = client_with_root("C:/definitely/not/a/real/replica/path/xyz", "proj:wg-1/dev");
        let err = resolve_from(&c).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn final_registry_bound_root_requires_the_same_filesystem_object() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("bound-root");
        let retired = temp.path().join("retired-root");
        std::fs::create_dir(&root).unwrap();
        let expected = crate::path_identity::verify_directory(&root).unwrap();
        let client = client_with_root(&root.to_string_lossy(), "project:wg-1-team/coordinator");
        assert!(verify_fresh_bound_root(&client, expected.object_id, &expected).is_some());

        std::fs::rename(&root, &retired).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(verify_fresh_bound_root(&client, expected.object_id, &expected).is_none());
    }

    #[test]
    fn wg_replica_root_resolves_to_fqn() {
        // Build a real WG-replica layout so agent_fqn_from_path yields a
        // non-root FQN and the existence check passes.
        let temp = tempfile::TempDir::new().unwrap();
        let replica = temp
            .path()
            .join("proj-x")
            .join(".ac")
            .join("wg-1-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let root = replica.to_string_lossy().to_string();
        let c = client_with_root(&root, "stale-hint");
        let from = resolve_from(&c).expect("existing replica should resolve");
        assert!(from.ends_with("/dev-rust"), "got: {}", from);
        assert_ne!(from, crate::config::root_agent::ROOT_AGENT_SENDER);
    }

    /// #1614 requirement (D), section 9.1 "API and snapshot fixture twins".
    ///
    /// This file takes ZERO production edits: it carries no prefix predicate
    /// and treats the FQN as opaque. That claim is what this twin turns from
    /// assumed into asserted. The `wg-` fixture above is deliberately NOT
    /// converted (Rule P2): a legacy fixture is the only way dual-prefix
    /// acceptance is testable at all, so the Room case is added BESIDE it.
    #[test]
    fn room_replica_root_resolves_to_fqn() {
        let temp = tempfile::TempDir::new().unwrap();
        let replica = temp
            .path()
            .join("proj-x")
            .join(".ac")
            .join("room-1-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let root = replica.to_string_lossy().to_string();
        let c = client_with_root(&root, "stale-hint");
        let from = resolve_from(&c).expect("existing Room replica should resolve");
        assert!(from.ends_with("/dev-rust"), "got: {}", from);
        assert!(
            from.contains("room-1-team"),
            "the FQN carries the literal directory name: {}",
            from
        );
        assert_ne!(from, crate::config::root_agent::ROOT_AGENT_SENDER);
    }

    /// #1614 (D): a Room replica and a legacy Workgroup replica at the same
    /// slot number are two DISTINCT peers, because every identity path keys on
    /// the full directory name and never on the prefix or the number (the
    /// mixed root of section 5.11, residual R1).
    #[test]
    fn room_and_legacy_replicas_at_the_same_slot_resolve_to_distinct_fqns() {
        let temp = tempfile::TempDir::new().unwrap();
        let ac = temp.path().join("proj-x").join(".ac");
        let mut resolved = Vec::new();
        for entity in ["wg-1-team", "room-1-team"] {
            let replica = ac.join(entity).join("__agent_dev-rust");
            std::fs::create_dir_all(&replica).unwrap();
            let c = client_with_root(&replica.to_string_lossy(), "stale-hint");
            resolved.push(resolve_from(&c).expect("replica should resolve"));
        }
        assert_ne!(
            resolved[0], resolved[1],
            "a Workgroup and a Room at slot 1 must be two different peers"
        );
        assert!(resolved[0].contains("wg-1-team"), "got: {}", resolved[0]);
        assert!(resolved[1].contains("room-1-team"), "got: {}", resolved[1]);
    }
}
