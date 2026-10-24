use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use crate::api::auth::{
    self, MintRequest, SCOPE_LIST_PEERS, SCOPE_PTY_INPUT, SCOPE_SEND, SCOPE_SESSION_TRANSPORT,
    SCOPE_TERMINAL_SNAPSHOT,
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
            scopes.push(SCOPE_TERMINAL_SNAPSHOT.to_string());
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
    /// (config.rs) while the label `com.agentscommander.session` is machine-wide
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
mod tests;
