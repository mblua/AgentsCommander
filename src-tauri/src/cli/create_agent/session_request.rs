use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config;
use crate::config::instance_artifacts::SESSION_REQUESTS_DIR_NAME;

/// Session request written to ~/.agentscommander/session-requests/.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub id: String,
    pub cwd: String,
    pub session_name: String,
    pub agent_id: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    pub timestamp: String,
}

pub(crate) fn find_launch_agent<'a>(
    settings: &'a crate::config::settings::AppSettings,
    requested: &str,
) -> Option<&'a crate::config::settings::AgentConfig> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }

    let requested_lower = requested.to_lowercase();
    settings.agents.iter().find(|a| {
        a.id.eq_ignore_ascii_case(requested)
            || a.label.eq_ignore_ascii_case(requested)
            || a.label.to_lowercase().contains(&requested_lower)
            || a.command.to_lowercase().starts_with(&requested_lower)
    })
}

pub(crate) fn build_session_request(
    cwd: String,
    session_name: String,
    agent: &crate::config::settings::AgentConfig,
) -> Result<SessionRequest, String> {
    let command = agent
        .command
        .trim_matches(|c: char| c.is_ascii_whitespace());
    if command.is_empty() {
        return Err(format!("launch agent '{}' has an empty command", agent.id));
    }

    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
        .map_err(|e| {
            format!(
                "launch agent '{}' has an invalid command: {}. command={:?}",
                agent.id, e, agent.command
            )
        })?;

    Ok(SessionRequest {
        id: uuid::Uuid::new_v4().to_string(),
        cwd,
        session_name,
        agent_id: agent.id.clone(),
        shell: normalized.shell,
        shell_args: normalized.shell_args,
        requested_profile: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

/// Write a session request file to ~/.agentscommander/session-requests/.
pub(crate) fn write_session_request(request: &SessionRequest) -> Result<(), String> {
    let config_dir = config::config_dir().ok_or("Cannot determine config directory")?;

    let requests_dir = config_dir.join(SESSION_REQUESTS_DIR_NAME);
    crate::filesystem::directories::ensure_cli_session_requests(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;

    let path = requests_dir.join(format!("{}.json", request.id));
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize session request: {}", e))?;
    crate::filesystem::documents::write_cli_session_document(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write session request: {}", e))?;

    Ok(())
}

/// #1163 - the outcome the running app recorded for a session request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionRequestResultStatus {
    Created,
    Rejected,
}

/// #1163 - the running app answers each session request with a result sidecar
/// (`<id>.result.json` in the session-requests dir) so the CLI can report a
/// truthful launch outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequestResult {
    pub id: String,
    pub status: SessionRequestResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `<config_dir>/session-requests/<id>.result.json`
pub(crate) fn session_request_result_path(config_dir: &Path, id: &str) -> PathBuf {
    config_dir
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{id}.result.json"))
}

/// True when `path` is a result sidecar (`<id>.result.json`), never a request.
pub(crate) fn is_session_request_result_path(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with(".result"))
}

/// Mirror of [`write_session_request`] for the result sidecar.
pub(crate) fn write_session_request_result(result: &SessionRequestResult) -> Result<(), String> {
    let config_dir = config::config_dir().ok_or("Cannot determine config directory")?;
    let requests_dir = config_dir.join(SESSION_REQUESTS_DIR_NAME);
    crate::filesystem::directories::ensure_cli_session_requests(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;
    let path = session_request_result_path(&config_dir, &result.id);
    let json = serde_json::to_string_pretty(result)
        .map_err(|e| format!("Failed to serialize session request result: {}", e))?;
    crate::filesystem::documents::write_cli_session_document(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write session request result: {}", e))?;
    Ok(())
}

/// Read and CONSUME `<id>.result.json`: on success parse and delete the file;
/// on missing or unparseable, delete-if-present and return `None`.
pub(crate) fn read_session_request_result(id: &str) -> Option<SessionRequestResult> {
    let config_dir = config::config_dir()?;
    let path = session_request_result_path(&config_dir, id);
    let content = match crate::filesystem::documents::read_cli_session_result(&path) {
        Ok(content) => content,
        Err(_) => return None,
    };
    match serde_json::from_str::<SessionRequestResult>(&content) {
        Ok(result) => {
            let _ = crate::filesystem::documents::remove_cli_session_artifact(&path);
            Some(result)
        }
        Err(_) => {
            let _ = crate::filesystem::documents::remove_cli_session_artifact(&path);
            None
        }
    }
}

/// #1163 - how long the CLI waits for the running app's result sidecar before
/// reporting `pending`. The app polls every 3 s, so a running app answers in
/// ~3-6 s plus spawn time; 30 s covers slow spawns without hanging scripts.
pub(crate) const SESSION_REQUEST_RESULT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);

/// #1163 - the CLI's truthful launch verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchOutcome {
    Pending,
    Launched { session_id: String },
    Rejected { error: String },
}

/// Poll [`read_session_request_result`] every 250 ms until `timeout` elapses;
/// `Created` → `Launched`, `Rejected` → `Rejected`, deadline → `Pending`.
pub(crate) fn wait_for_session_request_result(
    request_id: &str,
    timeout: std::time::Duration,
) -> LaunchOutcome {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(result) = read_session_request_result(request_id) {
            return match result.status {
                SessionRequestResultStatus::Created => LaunchOutcome::Launched {
                    session_id: result.session_id.unwrap_or_default(),
                },
                SessionRequestResultStatus::Rejected => LaunchOutcome::Rejected {
                    error: result.error.unwrap_or_default(),
                },
            };
        }
        if std::time::Instant::now() >= deadline {
            return LaunchOutcome::Pending;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
