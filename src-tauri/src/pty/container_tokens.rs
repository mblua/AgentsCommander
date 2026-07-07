use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use crate::api::auth::{self, MintRequest, SCOPE_LIST_PEERS, SCOPE_SEND, SCOPE_SESSION_TRANSPORT};
use crate::errors::AppError;

const CONTAINER_TOKEN_TTL_HOURS: i64 = 24;
const CONTAINER_LABEL_PREFIX: &str = "container:";

#[derive(Debug, Clone)]
pub struct ContainerApiToken {
    pub client_id: String,
    pub secret: String,
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
        let client_id = format!("container-{}", session_id);
        let secret = format!("ac-container-{}-{}", Uuid::new_v4(), Uuid::new_v4());
        let bound_fqn = crate::config::teams::agent_fqn_from_path(bound_root);
        let outcome = auth::mint(
            &self.registry_path,
            MintRequest {
                client_id: client_id.clone(),
                secret,
                label: format!("{}{}", CONTAINER_LABEL_PREFIX, session_id),
                bound_root: bound_root.to_string(),
                bound_fqn,
                scopes: vec![
                    SCOPE_SEND.to_string(),
                    SCOPE_LIST_PEERS.to_string(),
                    SCOPE_SESSION_TRANSPORT.to_string(),
                ],
                issued_at: issued_at.to_rfc3339(),
                expires_at: Some(expires_at.to_rfc3339()),
            },
        )
        .map_err(|e| AppError::Other(format!("failed to mint container API token: {e}")))?;

        Ok(ContainerApiToken {
            client_id: outcome.client_id,
            secret: outcome.secret,
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
}
