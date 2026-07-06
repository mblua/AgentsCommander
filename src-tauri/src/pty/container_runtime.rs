use std::collections::HashSet;
use std::time::Duration;

use uuid::Uuid;

use crate::errors::AppError;

pub const SESSION_LABEL: &str = "com.agentscommander.session";
pub const DEFAULT_CONTAINER_IMAGE: &str = "agentscommander/session-bridge:latest";
pub const DEFAULT_CONTAINER_WORKDIR: &str = "/workspace";
pub const DEFAULT_BRIDGE_ENTRYPOINT: &str = "session-bridge";
pub const DEFAULT_API_HELPER_PATH: &str = "/usr/local/bin/agentscommander-api-helper";
pub const CONTAINER_STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerStartRequest {
    pub session_id: Uuid,
    pub image: String,
    pub host_root: String,
    pub container_workdir: String,
    pub api_url: String,
    pub api_token: String,
    pub registration_ticket: String,
    pub local_dir: String,
    pub command: String,
    pub args: Vec<String>,
    pub child_env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRuntimeHandle {
    pub session_id: Uuid,
    pub container_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerCleanupReport {
    pub stopped: Vec<Uuid>,
    pub skipped_live: Vec<Uuid>,
    pub invalid_labels: Vec<String>,
}

pub trait ContainerRuntime: Send + Sync {
    fn start(&self, request: ContainerStartRequest) -> Result<ContainerRuntimeHandle, AppError>;

    fn stop(&self, handle: &ContainerRuntimeHandle, timeout: Duration) -> Result<(), AppError>;

    fn cleanup_labeled_orphans(
        &self,
        live_sessions: &HashSet<Uuid>,
        timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError>;
}

pub fn container_image_from_env() -> String {
    std::env::var("AGENTSCOMMANDER_CONTAINER_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CONTAINER_IMAGE.to_string())
}

pub fn api_url_for_container(bind: &str, port: u16) -> Result<String, AppError> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Err(AppError::Other(
            "container transport requires a non-empty API bind address".to_string(),
        ));
    }

    if bind == "127.0.0.1" || bind.eq_ignore_ascii_case("localhost") || bind == "::1" {
        return Err(AppError::Other(
            "container transport requires apiServerBind to be reachable from Docker, for example 0.0.0.0 with firewall restrictions".to_string(),
        ));
    }

    let host = match bind {
        "0.0.0.0" | "::" => "host.docker.internal",
        other => other,
    };
    Ok(format!("http://{}:{}", host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_rejects_loopback_and_maps_wildcard_to_docker_host() {
        assert!(api_url_for_container("127.0.0.1", 8765).is_err());
        assert_eq!(
            api_url_for_container("0.0.0.0", 8765).unwrap(),
            "http://host.docker.internal:8765"
        );
        assert_eq!(
            api_url_for_container("192.168.1.10", 8765).unwrap(),
            "http://192.168.1.10:8765"
        );
    }
}
