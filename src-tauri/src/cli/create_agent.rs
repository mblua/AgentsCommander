use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config;

#[derive(Args)]
pub struct CreateAgentArgs {
    /// Parent directory where the agent folder will be created
    #[arg(long)]
    pub parent: String,

    /// Name of the agent (will become a folder inside --parent)
    #[arg(long)]
    pub name: String,

    /// Coding agent to launch after creation (e.g., "claude", "codex").
    /// If omitted, the folder is created but no session is started.
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
    let parent = PathBuf::from(&args.parent);

    // Validate parent exists
    if !parent.exists() {
        eprintln!("Error: parent folder does not exist: {}", args.parent);
        return 1;
    }

    // Validate agent name
    let name = args.name.trim();
    if name.is_empty() {
        eprintln!("Error: --name cannot be empty");
        return 1;
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        eprintln!("Error: --name cannot contain path separators");
        return 1;
    }

    // Create agent directory
    let agent_dir = parent.join(name);
    if agent_dir.exists() {
        eprintln!("Error: folder already exists: {}", agent_dir.display());
        return 1;
    }

    if let Err(e) = std::fs::create_dir_all(&agent_dir) {
        eprintln!("Error: failed to create folder: {}", e);
        return 1;
    }

    // Derive display name: last component of parent / agent name
    let parent_name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| args.parent.clone());
    let full_agent_name = format!("{}/{}", parent_name, name);

    // Write CLAUDE.md
    let claude_content = format!("You are the agent {}", full_agent_name);
    let claude_path = agent_dir.join("CLAUDE.md");
    if let Err(e) = std::fs::write(&claude_path, &claude_content) {
        eprintln!("Error: failed to write CLAUDE.md: {}", e);
        return 1;
    }

    let agent_path_str = agent_dir.to_string_lossy().to_string();
    let mut launched = false;
    let mut launch_agent_id: Option<String> = None;

    // Handle --launch: write a session request for the running app to pick up
    if let Some(ref agent_id) = args.launch {
        let settings = config::settings::load_settings();

        let agent_config = settings.agents.iter().find(|a| {
            a.id.eq_ignore_ascii_case(agent_id)
                || a.label.eq_ignore_ascii_case(agent_id)
        });

        match agent_config {
            Some(agent) => {
                let parts: Vec<&str> = agent.command.trim().split_whitespace().collect();
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
                    session_name: full_agent_name.clone(),
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

    // Output result as JSON
    let result = CreateAgentResult {
        agent_path: agent_path_str,
        agent_name: full_agent_name,
        claude_md: claude_content,
        launched,
        launch_agent: launch_agent_id,
    };

    match serde_json::to_string_pretty(&result) {
        Ok(json) => println!("{}", json),
        Err(e) => {
            eprintln!("Error: failed to serialize result: {}", e);
            return 1;
        }
    }

    0
}

/// Write a session request file to ~/.agentscommander/session-requests/.
fn write_session_request(request: &SessionRequest) -> Result<(), String> {
    let config_dir = config::config_dir()
        .ok_or("Cannot determine config directory")?;

    let requests_dir = config_dir.join("session-requests");
    std::fs::create_dir_all(&requests_dir)
        .map_err(|e| format!("Failed to create session-requests dir: {}", e))?;

    let path = requests_dir.join(format!("{}.json", request.id));
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize session request: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write session request: {}", e))?;

    Ok(())
}
