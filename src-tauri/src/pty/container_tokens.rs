use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use crate::api::auth::{
    self, MintRequest, SCOPE_LIST_PEERS, SCOPE_PTY_INPUT, SCOPE_SEND, SCOPE_SESSION_TRANSPORT,
};
use crate::errors::AppError;

const CONTAINER_TOKEN_TTL_HOURS: i64 = 24;
const CONTAINER_LABEL_PREFIX: &str = "container:";

#[derive(Clone)]
pub struct ContainerApiToken {
    pub client_id: String,
    pub credential_generation: String,
    pub bound_session_id: String,
    pub secret: String,
    pub token_hash: String,
}

impl std::fmt::Debug for ContainerApiToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerApiToken")
            .field("client_id", &self.client_id)
            .field("credential_generation", &self.credential_generation)
            .field("bound_session_id", &self.bound_session_id)
            .field("secret", &"[REDACTED]")
            .field("token_hash", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ContainerApiTokenManager {
    registry_path: PathBuf,
}

impl ContainerApiTokenManager {
    pub fn at_config_dir() -> Option<Self> {
        crate::config::config_dir().map(|dir| Self {
            registry_path: dir.join(auth::REGISTRY_FILENAME),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_path(path: PathBuf) -> Self {
        Self {
            registry_path: path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.registry_path
    }

    pub fn mint_for_session(
        &self,
        session_id: Uuid,
        bound_root: &str,
    ) -> Result<ContainerApiToken, AppError> {
        let issued_at = Utc::now();
        let expires_at = issued_at + ChronoDuration::hours(CONTAINER_TOKEN_TTL_HOURS);
        let credential_generation = Uuid::new_v4().to_string();
        let bound_session_id = session_id.to_string();
        let client_id = format!("container-{}-{}", session_id, credential_generation);
        let secret = format!("ac-container-{}-{}", Uuid::new_v4(), Uuid::new_v4());
        let token_hash = auth::hash_token(&secret);
        let bound_fqn = crate::config::teams::agent_fqn_from_path(bound_root);
        let mut scopes = vec![
            SCOPE_SEND.to_string(),
            SCOPE_LIST_PEERS.to_string(),
            SCOPE_SESSION_TRANSPORT.to_string(),
        ];
        if crate::config::teams::verify_pty_input_coordinator_root(Path::new(bound_root)).is_ok() {
            scopes.push(SCOPE_PTY_INPUT.to_string());
        }
        let outcome = auth::mint(
            &self.registry_path,
            MintRequest {
                client_id: client_id.clone(),
                secret,
                label: format!("{}{}", CONTAINER_LABEL_PREFIX, session_id),
                bound_root: bound_root.to_string(),
                bound_fqn,
                scopes,
                issued_at: issued_at.to_rfc3339(),
                expires_at: Some(expires_at.to_rfc3339()),
                bound_session_id: Some(bound_session_id.clone()),
                credential_generation: Some(credential_generation.clone()),
            },
        )
        .map_err(|e| AppError::Other(format!("failed to mint container API token: {e}")))?;

        Ok(ContainerApiToken {
            client_id: outcome.client_id,
            credential_generation,
            bound_session_id,
            secret: outcome.secret,
            token_hash,
        })
    }

    pub fn revoke(&self, client_id: &str) {
        match auth::revoke(&self.registry_path, client_id) {
            Ok(true) => {
                log::info!("[container-token] revoked API client {}", client_id);
            }
            Ok(false) => {
                log::warn!(
                    "[container-token] API client {} was not found during revoke",
                    client_id
                );
            }
            Err(err) => {
                log::warn!(
                    "[container-token] failed to revoke API client {}: {}",
                    client_id,
                    err
                );
            }
        }
    }

    pub fn revoke_all_container_clients(&self) -> Result<usize, AppError> {
        let clients = auth::list(&self.registry_path).clients;
        let mut revoked = 0;
        let mut errors = Vec::new();
        for client in clients {
            if !client.label.starts_with(CONTAINER_LABEL_PREFIX) || client.revoked {
                continue;
            }
            match auth::revoke(&self.registry_path, &client.client_id) {
                Ok(true) => revoked += 1,
                Ok(false) => {}
                Err(err) => {
                    log::warn!(
                        "[container-token] failed to revoke stale container API client {}: {}",
                        client.client_id,
                        err
                    );
                    errors.push(format!("{}: {}", client.client_id, err));
                }
            }
        }
        if !errors.is_empty() {
            return Err(AppError::Other(format!(
                "failed to revoke {} container API client(s): {}",
                errors.len(),
                errors.join("; ")
            )));
        }
        Ok(revoked)
    }

    /// #992 - Has THIS config dir ever minted a container API client?
    ///
    /// SCOPED CLAIM. This answers "did this config dir ever create a labeled
    /// container", NOT "does this machine hold one". The registry is per config dir
    /// (config/mod.rs) while the label `com.agentscommander.session` is machine-wide
    /// (container_runtime.rs). Read plan 992 section 0 before you rely on this for
    /// anything.
    ///
    /// Within that scope it is exact, not a heuristic:
    ///   - a container session cannot spawn without a token manager
    ///     (container_backend.rs `spawn_runtime_backed` hard-errors),
    ///   - the client is minted BEFORE `docker run`, and `auth::mint` publishes
    ///     atomically to disk before it returns, so even a kill between the two
    ///     leaves the entry,
    ///   - the registry is append-only: `auth::revoke` only flips `revoked`, and
    ///     nothing prunes clients. (Assumption, not invariant: a concurrent CLI
    ///     process can lose an update, because `update_config_json_object` guards
    ///     with a process-local mutex only.)
    ///
    /// Revoked AND expired clients COUNT, deliberately. `expires_at` is issued_at +
    /// `CONTAINER_TOKEN_TTL_HOURS`, so an expired entry is the NORMAL state of an
    /// ex-container user, and a revoked entry still proves a container was created.
    /// `auth::list` filters neither - unlike `authenticate`, which rejects expired
    /// clients. If you ever "harmonize" the two, this predicate flips false for
    /// every ex-container user. Two tests pin that; do not delete them.
    ///
    /// `Err` = the registry could not be read (malformed/unreadable), so the caller
    /// can fail toward doing MORE work, never less. An ABSENT file is `Ok(false)`,
    /// not an error: `load_registry` maps only `NotFound` to a problem-free empty
    /// registry and every other IO error to `unreadable`.
    pub fn has_container_clients(&self) -> Result<bool, AppError> {
        let snapshot = auth::list_with_status(&self.registry_path);
        if let Some(problem) = snapshot.problem {
            return Err(AppError::Other(format!(
                "container API client registry at {} is unusable ({}): {}",
                self.registry_path.display(),
                problem.status,
                problem.message
            )));
        }
        Ok(snapshot
            .registry
            .clients
            .iter()
            .any(|client| client.label.starts_with(CONTAINER_LABEL_PREFIX)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_stores_hash_only_with_container_scopes_and_revoke_marks_client() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        let session_id = Uuid::new_v4();

        let token = manager
            .mint_for_session(session_id, "C:/project/.ac/wg-1-team/__agent_dev")
            .expect("mint");

        let raw = std::fs::read_to_string(manager.path()).expect("registry");
        assert!(!raw.contains(&token.secret));
        let registry = auth::list(manager.path());
        let client = registry
            .clients
            .iter()
            .find(|client| client.client_id == token.client_id)
            .expect("client");
        assert_eq!(client.label, format!("container:{}", session_id));
        assert!(client.scopes.contains(&SCOPE_SEND.to_string()));
        assert!(client.scopes.contains(&SCOPE_LIST_PEERS.to_string()));
        assert!(client.scopes.contains(&SCOPE_SESSION_TRANSPORT.to_string()));
        assert!(client.expires_at.is_some());

        manager.revoke(&token.client_id);
        let registry = auth::list(manager.path());
        assert!(registry.clients[0].revoked);
    }

    #[test]
    fn revoke_all_container_clients_only_revokes_container_labels() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        let one = manager
            .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
            .unwrap();
        auth::mint(
            manager.path(),
            MintRequest {
                client_id: "manual".to_string(),
                secret: "manual-secret".to_string(),
                label: "manual".to_string(),
                bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
                bound_fqn: "project:wg-1-team/dev".to_string(),
                scopes: vec![SCOPE_SEND.to_string()],
                issued_at: Utc::now().to_rfc3339(),
                expires_at: None,
                bound_session_id: None,
                credential_generation: None,
            },
        )
        .unwrap();

        assert_eq!(manager.revoke_all_container_clients().unwrap(), 1);
        let registry = auth::list(manager.path());
        assert!(
            registry
                .clients
                .iter()
                .find(|client| client.client_id == one.client_id)
                .unwrap()
                .revoked
        );
        assert!(
            !registry
                .clients
                .iter()
                .find(|client| client.client_id == "manual")
                .unwrap()
                .revoked
        );
    }

    fn manual_client(label: &str, expires_at: Option<String>) -> MintRequest {
        MintRequest {
            client_id: format!("client-{}", Uuid::new_v4()),
            secret: "manual-secret".to_string(),
            label: label.to_string(),
            bound_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
            bound_fqn: "project:wg-1-team/dev".to_string(),
            scopes: vec![SCOPE_SEND.to_string()],
            issued_at: Utc::now().to_rfc3339(),
            expires_at,
            bound_session_id: None,
            credential_generation: None,
        }
    }

    #[test]
    fn has_container_clients_is_false_when_registry_is_absent() {
        // The fresh-install path. If this ever returns true, every install pays a
        // load-bearing sweep forever.
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        assert!(!manager.has_container_clients().unwrap());
    }

    #[test]
    fn has_container_clients_is_true_after_minting_for_a_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        manager
            .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
            .unwrap();
        assert!(manager.has_container_clients().unwrap());
    }

    #[test]
    fn has_container_clients_stays_true_after_revoke_all_container_clients() {
        // Regression test for the stranding trap: the startup revoke runs even when the
        // sweep failed, so a marker that counted only NON-revoked clients would be
        // cleared by a start with Docker down. Do not delete this test.
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        manager
            .mint_for_session(Uuid::new_v4(), "C:/project/.ac/wg-1-team/__agent_dev")
            .unwrap();
        manager.revoke_all_container_clients().unwrap();
        assert!(manager.has_container_clients().unwrap());
    }

    /// Assert the registry really holds what a test claims it does. The first cut of
    /// these tests minted through `mint_for_session`, which stamps +24h, so they never
    /// expired anything and would have stayed green against a predicate that was broken
    /// on exactly the state they were named for.
    fn assert_client_is_expired(
        manager: &ContainerApiTokenManager,
        client_id: &str,
        revoked: bool,
    ) {
        let registry = auth::list(manager.path());
        let client = registry
            .clients
            .iter()
            .find(|client| client.client_id == client_id)
            .expect("the client the test minted");
        let expiry =
            chrono::DateTime::parse_from_rfc3339(client.expires_at.as_deref().expect("expires_at"))
                .expect("rfc3339 expiry")
                .with_timezone(&Utc);
        assert!(
            expiry < Utc::now(),
            "this test must actually expire the client"
        );
        assert_eq!(
            client.revoked, revoked,
            "this test must leave the client revoked={revoked}"
        );
    }

    fn mint_expired_container_client(manager: &ContainerApiTokenManager) -> String {
        let client_id = format!("container-{}", Uuid::new_v4());
        let mut request = manual_client(
            &format!("{}{}", CONTAINER_LABEL_PREFIX, Uuid::new_v4()),
            Some((Utc::now() - ChronoDuration::hours(48)).to_rfc3339()),
        );
        request.client_id = client_id.clone();
        request.issued_at = (Utc::now() - ChronoDuration::hours(72)).to_rfc3339();
        auth::mint(manager.path(), request).unwrap();
        client_id
    }

    #[test]
    fn has_container_clients_stays_true_for_an_expired_client() {
        // Container tokens live 24h (CONTAINER_TOKEN_TTL_HOURS), so an expired entry is
        // the normal state of an ex-container user. `auth::list` does not filter expired
        // entries; `authenticate` does. If someone ever harmonizes the two, this fails
        // before the marker silently flips false for every ex-container user.
        // `mint_for_session` cannot be used here: it stamps +24h.
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        let client_id = mint_expired_container_client(&manager);

        assert_client_is_expired(&manager, &client_id, false);
        assert!(manager.has_container_clients().unwrap());
    }

    #[test]
    fn has_container_clients_stays_true_for_an_expired_and_revoked_client() {
        // The ex-container user's REAL steady state: the token expired after 24h AND a
        // later startup revoked it. A predicate such as
        // `prefix && (!client.revoked || !is_expired(client))` passes the fresh+revoked
        // test and the expired-only test above, yet answers false here, silently
        // downgrading every ex-container user's sweep. That must not be possible.
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        let client_id = mint_expired_container_client(&manager);
        manager.revoke(&client_id);

        assert_client_is_expired(&manager, &client_id, true);
        assert!(manager.has_container_clients().unwrap());
    }

    #[test]
    fn has_container_clients_errors_on_a_malformed_registry() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("clients.json");
        std::fs::write(&path, "{").unwrap();
        let manager = ContainerApiTokenManager::new_for_path(path);
        assert!(manager.has_container_clients().is_err());
    }

    #[test]
    fn has_container_clients_ignores_non_container_clients() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        auth::mint(manager.path(), manual_client("cli:some-agent", None)).unwrap();
        assert!(!manager.has_container_clients().unwrap());
    }

    #[test]
    fn revoke_all_container_clients_is_a_noop_on_an_empty_registry() {
        // Pins the claim that a sweep which skips revocation skips nothing.
        let dir = tempfile::TempDir::new().unwrap();
        let manager = ContainerApiTokenManager::new_for_path(dir.path().join("clients.json"));
        assert_eq!(manager.revoke_all_container_clients().unwrap(), 0);
    }
}
