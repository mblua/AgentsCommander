use clap::Args;
use serde::{Deserialize, Serialize};

use crate::config::{self, agent_creation};

#[derive(Args)]
#[command(after_help = "\
WHAT IT DOES:\n  \
  1. Uses the same backend folder + CLAUDE.md creation helper as the UI modal\n  \
  2. Creates <parent>/<trimmed name>/ directory\n  \
  3. Writes CLAUDE.md with: \"You are the agent <parentFolder>/<trimmed name>\"\n  \
  4. If --launch is given, after folder creation writes a session request that the running app picks up (~3s)\n\n\
VALIDATION:\n  \
  --name is trimmed before use. It must not be empty after trim, and it must not contain path separators (/ or \\) or NUL.\n  \
  --parent must already exist; it is not created automatically.\n  \
  The target folder must not already exist; existing folders are not overwritten.\n\n\
OUTPUT: JSON object with fields: agentPath, agentName, claudeMd, launched, launchAgent.\n\n\
The agent name is derived as \"<last component of parent>/<trimmed name>\" (e.g., parent=\"C:\\repos\" + \
name=\" MyBot \" -> \"repos/MyBot\"). This is the name other agents will use with `send --to`.")]
pub struct CreateAgentArgs {
    /// Parent directory where the agent folder will be created
    #[arg(long)]
    pub parent: String,

    /// Name of the agent (becomes a subfolder inside --parent, and part of the agent name)
    #[arg(long)]
    pub name: String,

    /// Coding agent to launch after creation (e.g., "claude", "codex").
    /// Must match an agent id or label from settings.json. If omitted, the folder is created but no session is started
    #[arg(long)]
    pub launch: Option<String>,

    /// Agent root directory of the caller (for logging/context)
    #[arg(long)]
    pub root: Option<String>,

    /// Session token (for auth context)
    #[arg(long)]
    pub token: Option<String>,
}

/// JSON output printed to stdout on success.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentResult {
    agent_path: String,
    agent_name: String,
    claude_md: String,
    launched: bool,
    launch_agent: Option<String>,
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
    let command = agent.command.trim();
    if command.is_empty() {
        return Err(format!("launch agent '{}' has an empty command", agent.id));
    }

    let parts: Vec<&str> = command.split_whitespace().collect();
    let (shell, shell_args) = if agent.git_pull_before {
        (
            "cmd.exe".to_string(),
            vec!["/K".to_string(), format!("git pull && {}", command)],
        )
    } else {
        (
            parts.first().copied().unwrap_or_default().to_string(),
            parts
                .get(1..)
                .unwrap_or(&[])
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
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
    let created = match agent_creation::create_agent_folder_on_disk(&args.parent, &args.name) {
        Ok(created) => created,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let agent_path_str = created.agent_dir.to_string_lossy().to_string();
    let mut launched = false;
    let mut launch_agent_id: Option<String> = None;

    // Handle --launch: write a session request for the running app to pick up
    if let Some(ref agent_id) = args.launch {
        let settings = config::settings::load_settings();

        match find_launch_agent(&settings, agent_id) {
            Some(agent) => {
                // Auto-generate .claude/settings.local.json if the agent has the flag
                if agent.exclude_global_claude_md {
                    if let Err(e) =
                        config::claude_settings::ensure_claude_md_excludes(&created.agent_dir)
                    {
                        eprintln!("Warning: failed to write claude settings: {}", e);
                    }
                }
                // Issue #120 — apply the rtk hook based on the global toggle.
                // CLI runs out-of-process; cannot share the in-process RtkSweepLock
                // with a running AC instance. Cross-process race documented in §7.4
                // of the issue #120 plan as a follow-up.
                if let Err(e) = config::claude_settings::ensure_rtk_pretool_hook(
                    &created.agent_dir,
                    settings.inject_rtk_hook,
                ) {
                    eprintln!("Warning: failed to apply rtk hook: {}", e);
                }

                match build_session_request(
                    agent_path_str.clone(),
                    created.display_name.clone(),
                    agent,
                ) {
                    Ok(request) => match write_session_request(&request) {
                        Ok(()) => {
                            launched = true;
                            launch_agent_id = Some(agent.id.clone());
                        }
                        Err(e) => {
                            eprintln!("Warning: agent created but failed to request launch: {}", e);
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: agent created but failed to request launch: {}", e);
                    }
                }
            }
            None => {
                let available: Vec<&str> = settings.agents.iter().map(|a| a.id.as_str()).collect();
                eprintln!(
                    "Warning: agent '{}' not found in settings. Available: {}. Folder created but not launched.",
                    agent_id,
                    available.join(", ")
                );
            }
        }
    }

    let result = CreateAgentResult {
        agent_path: agent_path_str,
        agent_name: created.display_name,
        claude_md: created.claude_md,
        launched,
        launch_agent: launch_agent_id,
    };

    match serde_json::to_string_pretty(&result) {
        Ok(json) => crate::cli_println!("{}", json),
        Err(e) => {
            eprintln!("Error: failed to serialize result: {}", e);
            return 1;
        }
    }

    0
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
        let mut settings = AppSettings::default();
        settings.agents = vec![
            agent("codex", "OpenAI Codex", "codex"),
            agent(
                "claude",
                "Claude Desktop",
                "claude --dangerously-skip-permissions",
            ),
            agent("pwsh", "PowerShell", "powershell.exe -NoLogo"),
        ];

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
    }

    #[test]
    fn build_session_request_wraps_git_pull_before_with_cmd_on_windows_shape() {
        let mut launch_agent = agent("codex", "Codex", "codex --ask-for-approval never");
        launch_agent.git_pull_before = true;

        let request = build_session_request(
            "C:/repo/.ac-new/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .expect("request");

        assert_eq!(request.cwd, "C:/repo/.ac-new/_agent_architect");
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
    fn build_session_request_rejects_blank_and_whitespace_commands() {
        for command in ["", "   \t  "] {
            let launch_agent = agent("codex", "Codex", command);
            let err = build_session_request(
                "C:/repo/.ac-new/_agent_architect".to_string(),
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
            cwd: "C:/repo/.ac-new/_agent_architect".to_string(),
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
