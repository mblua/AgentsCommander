use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::cli::create_agent_matrix;
use crate::config;
use crate::config::instance_artifacts::SESSION_REQUESTS_DIR_NAME;

/// Test-only serialization for every test that writes or consumes files in
/// the process-shared `config_dir()/session-requests/` (the C1-C5 tests here
/// and the M1-M5 poll tests in `phone::mailbox`). `config_dir()` is a
/// process-wide `OnceLock`, so those tests share ONE directory; without the
/// lock a poll test could consume (or delete) another test's request/result
/// file mid-test.
#[cfg(test)]
pub(crate) static SESSION_REQUESTS_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[derive(Args)]
#[command(after_help = "\
WHAT IT DOES:\n  \
  Creates a full Agent Matrix in a registered AC project:\n  \
    agentscommander create-agent --project <PROJECT> --name <NAME> --description <DESC> [--role-template <TEMPLATE_ID>] [--launch <AGENT>]\n\n\
VALIDATION:\n  \
  --project is a registered AC project folder name from settings.projectPaths. Paths are not accepted.\n  \
  --name is trimmed before use. It must not be empty after trim, and it must not contain path separators (/ or \\) or NUL.\n\n\
OUTPUT:\n  \
  Prints the same JSON as create-agent-matrix: agentPath, agentName, rolePath, launched, launchStatus, launchError, launchAgent.")]
pub struct CreateAgentArgs {
    /// Registered AC project folder name. Paths are not accepted.
    #[arg(long, value_name = "PROJECT")]
    pub project: String,

    /// Name of the agent
    #[arg(long)]
    pub name: String,

    /// Description written into Role.md
    #[arg(long)]
    pub description: String,

    /// Optional role template id, for example agency:dev-rust or local:my-template
    #[arg(long = "role-template", value_name = "TEMPLATE_ID")]
    pub role_template: Option<String>,

    /// Coding agent to launch after creation (e.g., "claude", "codex").
    /// Must match an agent id or label from settings.json. If omitted, the Matrix is created but no session is started
    #[arg(long)]
    pub launch: Option<String>,

    /// Agent root directory of the caller (for logging/context)
    #[arg(long)]
    pub root: Option<String>,

    /// Session token (for auth context)
    #[arg(long)]
    pub token: Option<String>,
}

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

pub fn execute(args: CreateAgentArgs) -> i32 {
    let description = args.description.trim();
    if description.is_empty() {
        eprintln!("Error: --description must not be empty");
        return 1;
    }

    create_agent_matrix::execute_matrix_project_create(
        &args.project,
        &args.name,
        description,
        args.role_template.as_deref(),
        args.launch.as_deref(),
    )
}

/// Write a session request file to ~/.agentscommander/session-requests/.
pub(crate) fn write_session_request(request: &SessionRequest) -> Result<(), String> {
    let config_dir = config::config_dir().ok_or("Cannot determine config directory")?;

    let requests_dir = config_dir.join(SESSION_REQUESTS_DIR_NAME);
    std::fs::create_dir_all(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;

    let path = requests_dir.join(format!("{}.json", request.id));
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize session request: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write session request: {}", e))?;

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
    std::fs::create_dir_all(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;
    let path = session_request_result_path(&config_dir, &result.id);
    let json = serde_json::to_string_pretty(result)
        .map_err(|e| format!("Failed to serialize session request result: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write session request result: {}", e))?;
    Ok(())
}

/// Read and CONSUME `<id>.result.json`: on success parse and delete the file;
/// on missing or unparseable, delete-if-present and return `None`.
pub(crate) fn read_session_request_result(id: &str) -> Option<SessionRequestResult> {
    let config_dir = config::config_dir()?;
    let path = session_request_result_path(&config_dir, id);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return None,
    };
    match serde_json::from_str::<SessionRequestResult>(&content) {
        Ok(result) => {
            let _ = std::fs::remove_file(&path);
            Some(result)
        }
        Err(_) => {
            let _ = std::fs::remove_file(&path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{AgentConfig, AppSettings};

    fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }
    }

    #[test]
    fn find_launch_agent_matches_id_label_substring_and_command_prefix() {
        let settings = AppSettings {
            agents: vec![
                agent("codex", "OpenAI Codex", "codex"),
                agent(
                    "claude",
                    "Claude Desktop",
                    "claude --dangerously-skip-permissions",
                ),
                agent("pwsh", "PowerShell", "powershell.exe -NoLogo"),
            ],
            ..AppSettings::default()
        };

        assert_eq!(
            find_launch_agent(&settings, "CODEX").map(|a| a.id.as_str()),
            Some("codex")
        );
        assert_eq!(
            find_launch_agent(&settings, "Claude Desktop").map(|a| a.id.as_str()),
            Some("claude")
        );
        assert_eq!(
            find_launch_agent(&settings, "desktop").map(|a| a.id.as_str()),
            Some("claude")
        );
        assert_eq!(
            find_launch_agent(&settings, "powershell").map(|a| a.id.as_str()),
            Some("pwsh")
        );
        assert_eq!(
            find_launch_agent(&settings, "  CODEX  ").map(|a| a.id.as_str()),
            Some("codex")
        );
    }

    #[test]
    fn find_launch_agent_rejects_empty_and_whitespace_requests() {
        let settings = AppSettings {
            agents: vec![
                agent("codex", "OpenAI Codex", "codex"),
                agent("claude", "Claude Desktop", "claude"),
            ],
            ..AppSettings::default()
        };

        assert!(find_launch_agent(&settings, "").is_none());
        assert!(find_launch_agent(&settings, "   \t  ").is_none());
    }

    #[test]
    fn build_session_request_preserves_agent_args_from_command() {
        let launch_agent = agent("codex-yolo", "Codex Yolo", "codex --yolo");
        let request = build_session_request(
            "C:/repo/.ac/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .unwrap();

        assert_eq!(request.shell, "codex");
        assert_eq!(request.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn build_session_request_supports_quoted_executable_path() {
        let launch_agent = agent(
            "codex-local",
            "Codex Local",
            "\"C:\\Program Files\\Codex\\codex.exe\" --yolo",
        );
        let request = build_session_request(
            "C:/repo/.ac/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .unwrap();

        assert_eq!(request.shell, "C:\\Program Files\\Codex\\codex.exe");
        assert_eq!(request.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn build_session_request_rejects_invalid_quoted_command() {
        let launch_agent = agent("codex", "Codex", "codex \"unterminated");
        let err = build_session_request(
            "C:/repo/.ac/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .unwrap_err();

        assert!(err.contains("codex"));
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn build_session_request_rejects_blank_and_whitespace_commands() {
        for command in ["", "   \t  "] {
            let launch_agent = agent("codex", "Codex", command);
            let err = build_session_request(
                "C:/repo/.ac/_agent_architect".to_string(),
                "repo/architect".to_string(),
                &launch_agent,
            )
            .expect_err("blank command");

            assert!(err.contains("empty command"));
            assert!(err.contains("codex"));
        }
    }

    #[test]
    fn write_session_request_is_still_json_camel_case() {
        let _guard = SESSION_REQUESTS_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let request = SessionRequest {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            cwd: "C:/repo/.ac/_agent_architect".to_string(),
            session_name: "repo/architect".to_string(),
            agent_id: "codex".to_string(),
            shell: "codex".to_string(),
            shell_args: vec!["--ask-for-approval".to_string(), "never".to_string()],
            requested_profile: None,
            timestamp: "2026-05-28T00:00:00Z".to_string(),
        };

        write_session_request(&request).expect("write request");

        let path = crate::config::config_dir()
            .expect("config dir")
            .join(SESSION_REQUESTS_DIR_NAME)
            .join(format!("{}.json", request.id));
        let json = std::fs::read_to_string(&path).expect("read request");
        let _ = std::fs::remove_file(&path);

        assert!(json.contains("\"sessionName\""));
        assert!(json.contains("\"agentId\""));
        assert!(!json.contains("session_name"));
        assert!(!json.contains("agent_id"));
    }

    // #1163: the result sidecar must stay camelCase JSON so the CLI/app
    // contract is stable.
    #[test]
    fn write_session_request_result_is_still_json_camel_case() {
        let _guard = SESSION_REQUESTS_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let result = SessionRequestResult {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            status: SessionRequestResultStatus::Created,
            session_id: Some("sess-1".to_string()),
            error: None,
        };

        write_session_request_result(&result).expect("write result");

        let path = crate::config::config_dir()
            .expect("config dir")
            .join(SESSION_REQUESTS_DIR_NAME)
            .join(format!("{}.result.json", result.id));
        let json = std::fs::read_to_string(&path).expect("read result");
        let _ = std::fs::remove_file(&path);

        assert!(json.contains("\"status\""));
        assert!(json.contains("\"sessionId\""));
        assert!(!json.contains("session_id"));
        assert!(!json.contains("created_at"));
    }

    #[test]
    fn read_session_request_result_consumes_and_deletes() {
        let _guard = SESSION_REQUESTS_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let result = SessionRequestResult {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            status: SessionRequestResultStatus::Created,
            session_id: Some("sess-1".to_string()),
            error: None,
        };
        write_session_request_result(&result).expect("write result");
        let path = crate::config::config_dir()
            .expect("config dir")
            .join(SESSION_REQUESTS_DIR_NAME)
            .join(format!("{}.result.json", result.id));

        let read = read_session_request_result(&result.id).expect("read result");
        assert_eq!(read.status, SessionRequestResultStatus::Created);
        assert_eq!(read.session_id.as_deref(), Some("sess-1"));
        assert!(!path.exists(), "consumed result must be deleted");
    }

    #[test]
    fn wait_for_session_request_result_returns_launched() {
        let _guard = SESSION_REQUESTS_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let result = SessionRequestResult {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            status: SessionRequestResultStatus::Created,
            session_id: Some("sess-1".to_string()),
            error: None,
        };
        write_session_request_result(&result).expect("write result");

        let outcome =
            wait_for_session_request_result(&result.id, std::time::Duration::from_secs(1));
        assert_eq!(
            outcome,
            LaunchOutcome::Launched {
                session_id: "sess-1".to_string()
            }
        );
    }

    #[test]
    fn wait_for_session_request_result_returns_rejected() {
        let _guard = SESSION_REQUESTS_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let result = SessionRequestResult {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            status: SessionRequestResultStatus::Rejected,
            session_id: None,
            error: Some("sessionRace".to_string()),
        };
        write_session_request_result(&result).expect("write result");

        let outcome =
            wait_for_session_request_result(&result.id, std::time::Duration::from_secs(1));
        assert_eq!(
            outcome,
            LaunchOutcome::Rejected {
                error: "sessionRace".to_string()
            }
        );
    }

    #[test]
    fn wait_for_session_request_result_times_out_as_pending() {
        let id = format!("test-{}", uuid::Uuid::new_v4().simple());
        let outcome = wait_for_session_request_result(&id, std::time::Duration::from_millis(300));
        assert_eq!(outcome, LaunchOutcome::Pending);
    }
}
