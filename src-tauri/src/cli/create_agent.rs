use clap::Args;
use serde::{Deserialize, Serialize};

use crate::cli::create_agent_matrix;
use crate::config;

#[derive(Args)]
#[command(after_help = "\
WHAT IT DOES:\n  \
  Creates a full Agent Matrix in a registered AC project:\n  \
    agentscommander create-agent --project <PROJECT> --name <NAME> --description <DESC> [--role-template <TEMPLATE_ID>] [--launch <AGENT>]\n\n\
VALIDATION:\n  \
  --project is a registered AC project folder name from settings.projectPaths. Paths are not accepted.\n  \
  --name is trimmed before use. It must not be empty after trim, and it must not contain path separators (/ or \\) or NUL.\n\n\
OUTPUT:\n  \
  Prints the same JSON as create-agent-matrix: agentPath, agentName, rolePath, launched, launchAgent.")]
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

    let (shell, shell_args) = if agent.git_pull_before {
        (
            "cmd.exe".to_string(),
            vec!["/K".to_string(), format!("git pull && {}", command)],
        )
    } else {
        let normalized = crate::config::agent_command::normalize_legacy_agent_command(command)
            .map_err(|e| {
                format!(
                    "launch agent '{}' has an invalid command: {}. command={:?}",
                    agent.id, e, agent.command
                )
            })?;
        (normalized.shell, normalized.shell_args)
    };

    Ok(SessionRequest {
        id: uuid::Uuid::new_v4().to_string(),
        cwd,
        session_name,
        agent_id: agent.id.clone(),
        shell,
        shell_args,
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

    let requests_dir = config_dir.join("session-requests");
    std::fs::create_dir_all(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;

    let path = requests_dir.join(format!("{}.json", request.id));
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize session request: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write session request: {}", e))?;

    Ok(())
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
            git_pull_before: false,
            exclude_global_claude_md: false,
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
    fn build_session_request_wraps_git_pull_before_with_cmd_on_windows_shape() {
        let mut launch_agent = agent("codex", "Codex", "codex --ask-for-approval never");
        launch_agent.git_pull_before = true;

        let request = build_session_request(
            "C:/repo/.ac/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .expect("request");

        assert_eq!(request.cwd, "C:/repo/.ac/_agent_architect");
        assert_eq!(request.session_name, "repo/architect");
        assert_eq!(request.agent_id, "codex");
        assert_eq!(request.shell, "cmd.exe");
        assert_eq!(
            request.shell_args,
            vec![
                "/K".to_string(),
                "git pull && codex --ask-for-approval never".to_string()
            ]
        );
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
        let request = SessionRequest {
            id: format!("test-{}", uuid::Uuid::new_v4().simple()),
            cwd: "C:/repo/.ac/_agent_architect".to_string(),
            session_name: "repo/architect".to_string(),
            agent_id: "codex".to_string(),
            shell: "codex".to_string(),
            shell_args: vec!["--ask-for-approval".to_string(), "never".to_string()],
            timestamp: "2026-05-28T00:00:00Z".to_string(),
        };

        write_session_request(&request).expect("write request");

        let path = crate::config::config_dir()
            .expect("config dir")
            .join("session-requests")
            .join(format!("{}.json", request.id));
        let json = std::fs::read_to_string(&path).expect("read request");
        let _ = std::fs::remove_file(&path);

        assert!(json.contains("\"sessionName\""));
        assert!(json.contains("\"agentId\""));
        assert!(!json.contains("session_name"));
        assert!(!json.contains("agent_id"));
    }
}
