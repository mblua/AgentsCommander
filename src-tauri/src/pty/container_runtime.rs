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
const REDACTED_SECRET: &str = "[REDACTED]";
// Keep AGENTSCOMMANDER_* entries in sync with DockerRuntime::build_run_command.
// This list is separate because diagnostics must also scrub valid provider env keys.
const SENSITIVE_LOG_KEYS: &[&str] = &[
    "AGENTSCOMMANDER_API_TOKEN",
    "AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_TOKEN",
    "CLAUDE_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "GEMINI_API_KEY",
    "GITHUB_TOKEN",
    "OPENAI_API_KEY",
];
const SENSITIVE_TOKEN_PREFIXES: &[&str] = &[
    "ac-container-",
    "acst-",
    "AIza",
    "ghp_",
    "sk-ant-",
    "sk-proj-",
];

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
    pub env_unset: Vec<String>,
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
        match self.log_tail.as_deref() {
            Some(log_tail) => {
                let redacted = redact_container_diagnostic_text(log_tail);
                let compact = compact_log_tail_bounded(&redacted, DIAGNOSTIC_UI_LOG_LIMIT);
                if !compact.is_empty() {
                    summary.push_str("; logs: ");
                    summary.push_str(&compact);
                } else if let Some(err) = self.logs_error.as_deref() {
                    summary.push_str("; logs unavailable: ");
                    summary.push_str(&redact_and_truncate(err, DIAGNOSTIC_UI_LOG_LIMIT));
                } else {
                    summary.push_str("; logs: <empty>");
                }
            }
            None => {
                if let Some(err) = self.logs_error.as_deref() {
                    summary.push_str("; logs unavailable: ");
                    summary.push_str(&redact_and_truncate(err, DIAGNOSTIC_UI_LOG_LIMIT));
                }
            }
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
                let redacted = redact_container_diagnostic_text(log_tail);
                summary.push_str("\ncontainer log tail:\n");
                summary.push_str(redacted.trim_end());
            }
            _ => {
                summary.push_str("\ncontainer log tail: <empty>");
            }
        }
        if let Some(err) = self.logs_error.as_deref() {
            summary.push_str("\nlogs error: ");
            summary.push_str(&redact_container_diagnostic_text(err));
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

fn compact_log_tail_bounded(log_tail: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut truncated = false;
    for token in log_tail.split_whitespace() {
        if !out.is_empty() && !push_chars_bounded(&mut out, &mut count, " ", max_chars) {
            truncated = true;
            break;
        }
        if !push_chars_bounded(&mut out, &mut count, token, max_chars) {
            truncated = true;
            break;
        }
    }
    if truncated {
        out.push_str("...");
    }
    out
}

fn push_chars_bounded(out: &mut String, count: &mut usize, text: &str, max_chars: usize) -> bool {
    for ch in text.chars() {
        if *count >= max_chars {
            return false;
        }
        out.push(ch);
        *count += 1;
    }
    true
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

fn redact_and_truncate(value: &str, max_chars: usize) -> String {
    truncate_chars(&redact_container_diagnostic_text(value), max_chars)
}

fn redact_container_diagnostic_text(input: &str) -> String {
    let mut redacted = input.to_string();
    for key in SENSITIVE_LOG_KEYS {
        redacted = redact_key_values(&redacted, key);
    }
    for prefix in SENSITIVE_TOKEN_PREFIXES {
        redacted = redact_token_prefix(&redacted, prefix);
    }
    redacted
}

fn redact_key_values(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let key_lower = key.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut ranges = Vec::new();
    let mut search_start = 0;

    while let Some(relative) = lower[search_start..].find(&key_lower) {
        let key_start = search_start + relative;
        let mut cursor = key_start + key.len();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let mut quoted_key = false;
        if cursor < bytes.len() && (bytes[cursor] == b'"' || bytes[cursor] == b'\'') {
            quoted_key = true;
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
        }
        if quoted_key && cursor < bytes.len() && bytes[cursor] == b',' {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= bytes.len() || !matches!(bytes[cursor], b'"' | b'\'') {
                search_start = key_start + key.len();
                continue;
            }
        } else if cursor < bytes.len() && matches!(bytes[cursor], b'=' | b':') {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
        } else {
            search_start = key_start + key.len();
            continue;
        }

        let (value_start, value_end) =
            if cursor < bytes.len() && (bytes[cursor] == b'"' || bytes[cursor] == b'\'') {
                let quote = bytes[cursor];
                let value_start = cursor + 1;
                let mut value_end = value_start;
                while value_end < bytes.len() && bytes[value_end] != quote {
                    value_end += 1;
                }
                (value_start, value_end)
            } else {
                let value_start = cursor;
                let mut value_end = value_start;
                while value_end < bytes.len() {
                    if is_unquoted_secret_delimiter(bytes[value_end]) {
                        break;
                    }
                    let ch = input[value_end..]
                        .chars()
                        .next()
                        .expect("valid char boundary");
                    value_end += ch.len_utf8();
                }
                (value_start, value_end)
            };

        if value_end > value_start {
            ranges.push((value_start, value_end));
        }
        search_start = value_end.max(key_start + key.len());
    }

    if ranges.is_empty() {
        return input.to_string();
    }

    let mut out = input.to_string();
    for (start, end) in ranges.into_iter().rev() {
        out.replace_range(start..end, REDACTED_SECRET);
    }
    out
}

fn is_unquoted_secret_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b',' | b';' | b'}' | b']')
}

fn redact_token_prefix(input: &str, prefix: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;

    while cursor < input.len() {
        if input[cursor..].starts_with(prefix) {
            let mut end = cursor + prefix.len();
            while end < bytes.len() && is_token_char(bytes[end]) {
                end += 1;
            }
            if end > cursor + prefix.len() {
                out.push_str(REDACTED_SECRET);
                cursor = end;
                continue;
            }
        }

        let ch = input[cursor..].chars().next().expect("valid char boundary");
        out.push(ch);
        cursor += ch.len_utf8();
    }

    out
}

fn is_token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
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
        let long_tail = "x".repeat(DIAGNOSTIC_UI_LOG_LIMIT + 25);
        let diagnostics = ContainerDiagnostics {
            container_id: "abc123".to_string(),
            state: Some(ContainerStateSnapshot {
                status: Some("exited".to_string()),
                running: Some(false),
                exit_code: Some(127),
                error: None,
            }),
            inspect_error: None,
            log_tail: Some(format!("line one\n{long_tail}")),
            logs_error: None,
        };

        let ui = diagnostics.ui_summary();
        assert!(ui.contains("container id abc123"), "{ui}");
        assert!(ui.contains("status=exited"), "{ui}");
        assert!(ui.contains("exitCode=127"), "{ui}");
        let ui_logs = ui.split("logs: ").nth(1).expect("ui logs");
        assert!(ui_logs.ends_with("..."), "{ui}");
        assert!(
            ui_logs.chars().count() <= DIAGNOSTIC_UI_LOG_LIMIT + 3,
            "{ui}"
        );

        let full = diagnostics.log_summary();
        assert!(full.contains(&long_tail), "{full}");
    }

    #[test]
    fn container_diagnostics_redacts_tokens_from_ui_and_log_summary() {
        let api_token =
            "ac-container-11111111-1111-4111-8111-111111111111-22222222-2222-4222-8222-222222222222";
        let registration_ticket =
            "acst-33333333-3333-4333-8333-333333333333-44444444-4444-4444-8444-444444444444";
        let openai_key = "openai-secret-without-sensitive-prefix";
        let gemini_key = "gemini-secret-without-sensitive-prefix";
        let github_token = "github-token-without-sensitive-prefix";
        let bare_openai_key = "sk-proj-bare-openai-secret";
        let bare_gemini_key = "AIzaBareGeminiSecret";
        let bare_github_token = "ghp_bare_github_secret";
        let child_env = vec![
            ("OPENAI_API_KEY".to_string(), openai_key.to_string()),
            ("GEMINI_API_KEY".to_string(), gemini_key.to_string()),
            ("GITHUB_TOKEN".to_string(), github_token.to_string()),
            ("PATH".to_string(), "/usr/local/bin".to_string()),
        ];
        let child_env_json = serde_json::to_string(&child_env).expect("serialize child env");
        let bridge_args = vec![
            bare_openai_key,
            bare_gemini_key,
            bare_github_token,
            "--verbose",
        ];
        let bridge_args_json = serde_json::to_string(&bridge_args).expect("serialize bridge args");
        let diagnostics = ContainerDiagnostics {
            container_id: "abc123".to_string(),
            state: None,
            inspect_error: Some("inspect failed".to_string()),
            log_tail: Some(format!(
                "AGENTSCOMMANDER_API_TOKEN={api_token}\n\
                 AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN={registration_ticket}\n\
                 AGENTSCOMMANDER_BRIDGE_ARGS_JSON={bridge_args_json}\n\
                 AGENTSCOMMANDER_BRIDGE_ENV_JSON={child_env_json}"
            )),
            logs_error: Some(format!(
                "failed after token {api_token} and ticket {registration_ticket}"
            )),
        };

        let ui = diagnostics.ui_summary();
        let full = diagnostics.log_summary();

        for secret in [
            api_token,
            registration_ticket,
            openai_key,
            gemini_key,
            github_token,
            bare_openai_key,
            bare_gemini_key,
            bare_github_token,
        ] {
            assert!(!ui.contains(secret), "{ui}");
            assert!(!full.contains(secret), "{full}");
        }
        assert!(ui.contains(REDACTED_SECRET), "{ui}");
        assert!(full.contains(REDACTED_SECRET), "{full}");
        assert!(ui.contains("PATH"), "{ui}");
        assert!(ui.contains("/usr/local/bin"), "{ui}");
        assert!(full.contains("PATH"), "{full}");
        assert!(full.contains("/usr/local/bin"), "{full}");
        assert!(ui.contains("--verbose"), "{ui}");
        assert!(full.contains("--verbose"), "{full}");
    }

    #[test]
    fn ui_summary_reports_empty_tail_logs_error_bounded() {
        let token = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789";
        let diagnostics = ContainerDiagnostics {
            container_id: "abc123".to_string(),
            state: None,
            inspect_error: None,
            log_tail: Some(String::new()),
            logs_error: Some(format!(
                "docker logs failed CLAUDE_CODE_OAUTH_TOKEN={token} {}",
                "e".repeat(DIAGNOSTIC_UI_LOG_LIMIT + 25)
            )),
        };

        let ui = diagnostics.ui_summary();
        let ui_error = ui.split("logs unavailable: ").nth(1).expect("logs error");

        assert!(ui.contains("logs unavailable"), "{ui}");
        assert!(!ui.contains(token), "{ui}");
        assert!(ui.contains(REDACTED_SECRET), "{ui}");
        assert!(
            ui_error.chars().count() <= DIAGNOSTIC_UI_LOG_LIMIT + 3,
            "{ui}"
        );
    }
}
