use std::collections::HashSet;
use std::time::Duration;

use uuid::Uuid;

use crate::errors::AppError;

pub const SESSION_LABEL: &str = "com.agentscommander.session";
pub const DEFAULT_CONTAINER_WORKDIR: &str = "/workspace";
pub const DEFAULT_BRIDGE_ENTRYPOINT: &str = "session-bridge";
pub const DEFAULT_API_HELPER_PATH: &str = "/usr/local/bin/agentscommander-api-helper";
pub const CONTAINER_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_UI_LOG_LIMIT: usize = 500;

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerStateSnapshot {
    pub status: Option<String>,
    pub running: Option<bool>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerDiagnostics {
    pub container_id: String,
    pub state: Option<ContainerStateSnapshot>,
    pub inspect_error: Option<String>,
    pub log_tail: Option<String>,
    pub logs_error: Option<String>,
}

impl ContainerDiagnostics {
    pub fn unavailable(
        handle: &ContainerRuntimeHandle,
        error: impl Into<String>,
    ) -> ContainerDiagnostics {
        ContainerDiagnostics {
            container_id: handle.container_id.clone(),
            state: None,
            inspect_error: Some(error.into()),
            log_tail: None,
            logs_error: None,
        }
    }

    pub fn ui_summary(&self) -> String {
        let mut summary = self.state_summary();
        if let Some(log_tail) = self.log_tail.as_deref().map(compact_log_tail) {
            if !log_tail.is_empty() {
                summary.push_str("; logs: ");
                summary.push_str(&truncate_chars(&log_tail, DIAGNOSTIC_UI_LOG_LIMIT));
            }
        } else if let Some(err) = self.logs_error.as_deref() {
            summary.push_str("; logs unavailable: ");
            summary.push_str(err);
        }
        summary
    }

    pub fn log_summary(&self) -> String {
        let mut summary = self.state_summary();
        if let Some(err) = self.inspect_error.as_deref() {
            summary.push_str("\ninspect error: ");
            summary.push_str(err);
        }
        match self.log_tail.as_deref() {
            Some(log_tail) if !log_tail.trim().is_empty() => {
                summary.push_str("\ncontainer log tail:\n");
                summary.push_str(log_tail.trim_end());
            }
            _ => {
                summary.push_str("\ncontainer log tail: <empty>");
            }
        }
        if let Some(err) = self.logs_error.as_deref() {
            summary.push_str("\nlogs error: ");
            summary.push_str(err);
        }
        summary
    }

    fn state_summary(&self) -> String {
        let mut parts = vec![format!("container id {}", self.container_id)];
        if let Some(state) = &self.state {
            if let Some(status) = state.status.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("status={status}"));
            }
            if let Some(running) = state.running {
                parts.push(format!("running={running}"));
            }
            if let Some(exit_code) = state.exit_code {
                parts.push(format!("exitCode={exit_code}"));
            }
            if let Some(error) = state.error.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("stateError={error}"));
            }
        } else if self.inspect_error.is_some() {
            parts.push("state unavailable".to_string());
        }
        parts.join(", ")
    }
}

pub trait ContainerRuntime: Send + Sync {
    fn start(&self, request: ContainerStartRequest) -> Result<ContainerRuntimeHandle, AppError>;

    fn stop(&self, handle: &ContainerRuntimeHandle, timeout: Duration) -> Result<(), AppError>;

    fn diagnostics(
        &self,
        handle: &ContainerRuntimeHandle,
        _log_tail_lines: usize,
    ) -> ContainerDiagnostics {
        ContainerDiagnostics::unavailable(handle, "container runtime diagnostics are not supported")
    }

    fn cleanup_labeled_orphans(
        &self,
        live_sessions: &HashSet<Uuid>,
        timeout: Duration,
    ) -> Result<ContainerCleanupReport, AppError>;
}

pub fn container_image_from_env() -> Option<String> {
    std::env::var("AGENTSCOMMANDER_CONTAINER_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn resolve_container_image(per_agent: Option<&str>) -> Result<String, AppError> {
    match per_agent.map(str::trim).filter(|image| !image.is_empty()) {
        Some(image) => Ok(image.to_string()),
        None => container_image_from_env().ok_or_else(|| {
            AppError::Other(
                "containerTransport requires backend.image (container image); set the agent's backend.image field or AGENTSCOMMANDER_CONTAINER_IMAGE before launch".to_string(),
            )
        }),
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

fn compact_log_tail(log_tail: &str) -> String {
    log_tail.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
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
    fn resolve_container_image_uses_per_agent_then_env() {
        with_container_image_env(Some("env/image:latest"), || {
            assert_eq!(
                resolve_container_image(Some(" per-agent/image:latest ")).unwrap(),
                "per-agent/image:latest"
            );
            assert_eq!(
                resolve_container_image(Some("  ")).unwrap(),
                "env/image:latest"
            );
            assert_eq!(resolve_container_image(None).unwrap(), "env/image:latest");
        });

        with_container_image_env(Some("  "), || {
            let err = resolve_container_image(None).unwrap_err().to_string();
            assert!(err.contains("backend.image"), "{err}");
        });

        with_container_image_env(None, || {
            let err = resolve_container_image(None).unwrap_err().to_string();
            assert!(err.contains("AGENTSCOMMANDER_CONTAINER_IMAGE"), "{err}");
        });
    }

    #[test]
    fn container_diagnostics_formats_bounded_ui_and_full_log() {
        let diagnostics = ContainerDiagnostics {
            container_id: "abc123".to_string(),
            state: Some(ContainerStateSnapshot {
                status: Some("exited".to_string()),
                running: Some(false),
                exit_code: Some(127),
                error: None,
            }),
            inspect_error: None,
            log_tail: Some("line one\nline two".to_string()),
            logs_error: None,
        };

        let ui = diagnostics.ui_summary();
        assert!(ui.contains("container id abc123"), "{ui}");
        assert!(ui.contains("status=exited"), "{ui}");
        assert!(ui.contains("exitCode=127"), "{ui}");
        assert!(ui.contains("logs: line one line two"), "{ui}");

        let full = diagnostics.log_summary();
        assert!(
            full.contains("container log tail:\nline one\nline two"),
            "{full}"
        );
    }
}
