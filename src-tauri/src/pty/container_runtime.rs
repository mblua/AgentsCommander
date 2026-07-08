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

pub fn resolve_container_image(per_agent: Option<&str>) -> String {
    match per_agent.map(str::trim).filter(|image| !image.is_empty()) {
        Some(image) => image.to_string(),
        None => container_image_from_env(),
    }
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_container_image_env(value: Option<&str>, test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("AGENTSCOMMANDER_CONTAINER_IMAGE");
        match value {
            Some(value) => std::env::set_var("AGENTSCOMMANDER_CONTAINER_IMAGE", value),
            None => std::env::remove_var("AGENTSCOMMANDER_CONTAINER_IMAGE"),
        }
        test();
        match previous {
            Some(value) => std::env::set_var("AGENTSCOMMANDER_CONTAINER_IMAGE", value),
            None => std::env::remove_var("AGENTSCOMMANDER_CONTAINER_IMAGE"),
        }
    }

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

    #[test]
    fn resolve_container_image_uses_per_agent_env_then_default() {
        with_container_image_env(Some("env/image:latest"), || {
            assert_eq!(
                resolve_container_image(Some(" per-agent/image:latest ")),
                "per-agent/image:latest"
            );
            assert_eq!(resolve_container_image(Some("  ")), "env/image:latest");
            assert_eq!(resolve_container_image(None), "env/image:latest");
        });

        with_container_image_env(Some("  "), || {
            assert_eq!(resolve_container_image(None), DEFAULT_CONTAINER_IMAGE);
        });

        with_container_image_env(None, || {
            assert_eq!(resolve_container_image(None), DEFAULT_CONTAINER_IMAGE);
        });
    }
}
