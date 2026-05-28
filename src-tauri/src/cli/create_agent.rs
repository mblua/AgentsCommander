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

        let agent_id_lower = agent_id.to_lowercase();
        let agent_config = settings.agents.iter().find(|a| {
            a.id.eq_ignore_ascii_case(agent_id)
                || a.label.eq_ignore_ascii_case(agent_id)
                || a.label.to_lowercase().contains(&agent_id_lower)
                || a.command.to_lowercase().starts_with(&agent_id_lower)
        });

        match agent_config {
            Some(agent) => {
                // Auto-generate .claude/settings.local.json if the agent has the flag
                if agent.exclude_global_claude_md {
                    if let Err(e) =
                        config::claude_settings::ensure_claude_md_excludes(&created.agent_dir)
                    {
                        eprintln!("Warning: failed to write claude settings: {}", e);
                    }
                }
                // Issue #120 - apply the rtk hook based on the global toggle.
                // CLI runs out-of-process; cannot share the in-process RtkSweepLock
                // with a running AC instance. Cross-process race documented in §7.4
                // of the issue #120 plan as a follow-up.
                if let Err(e) = config::claude_settings::ensure_rtk_pretool_hook(
                    &created.agent_dir,
                    settings.inject_rtk_hook,
                ) {
                    eprintln!("Warning: failed to apply rtk hook: {}", e);
                }

                let parts: Vec<&str> = agent.command.split_whitespace().collect();
                let (shell, shell_args) = if agent.git_pull_before {
                    (
                        "cmd.exe".to_string(),
                        vec!["/K".to_string(), format!("git pull && {}", agent.command)],
                    )
                } else {
                    (
                        parts[0].to_string(),
                        parts[1..].iter().map(|s| s.to_string()).collect(),
                    )
                };

                let request = SessionRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    cwd: agent_path_str.clone(),
                    session_name: created.display_name.clone(),
                    agent_id: agent.id.clone(),
                    shell,
                    shell_args,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };

                match write_session_request(&request) {
                    Ok(()) => {
                        launched = true;
                        launch_agent_id = Some(agent.id.clone());
                    }
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
fn write_session_request(request: &SessionRequest) -> Result<(), String> {
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
