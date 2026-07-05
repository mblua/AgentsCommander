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
        }
    }

    #[test]
    fn missing_bound_root_is_401() {
        let c = client_with_root(
            "C:/definitely/not/a/real/replica/path/xyz",
            "proj:wg-1/dev",
        );
        let err = resolve_from(&c).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::UNAUTHORIZED);
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
}
