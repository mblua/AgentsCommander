//! CLI entrypoint for creating Agent Matrices.
//!
//! This command runs out of process, so it reads the persisted settings snapshot
//! with `load_settings_for_cli()` and cannot share the GUI's in-memory
//! `SettingsState` or RTK sweep lock. That matches the existing CLI mutation
//! model: local disk creation is immediate, while launch requests are handed to
//! the running GUI through the session-request mailbox.

use clap::Args;
use serde::Serialize;

use crate::cli::create_agent;
use crate::commands::entity_creation::{
    apply_agent_matrix_settings_files, create_agent_matrix_on_disk, AgentMatrixSettingsFlags,
    CreateAgentMatrixDiskArgs,
};

#[derive(Args)]
#[command(after_help = "\
NOTES:\n  \
  .ac-new must already exist under --project.\n  \
  --name is sanitized by the same backend as the UI into a lower-case Matrix id.\n  \
  --role-template must be an id from the same source as the New Agent picker.\n  \
  Invalid templates fail before creating the target Matrix directory.\n  \
  Output is JSON with agentPath, agentName, rolePath, launched, launchAgent.")]
pub struct CreateAgentMatrixArgs {
    /// AC project directory that already contains .ac-new
    #[arg(long, value_name = "PATH")]
    pub project: String,

    /// Agent Matrix display/input name, sanitized into the _agent_<id> folder
    #[arg(long)]
    pub name: String,

    /// Description written into Role.md frontmatter and body
    #[arg(long)]
    pub description: String,

    /// Optional role template id, for example agency:dev-rust or local:my-template
    #[arg(long = "role-template", value_name = "TEMPLATE_ID")]
    pub role_template: Option<String>,

    /// Coding agent to launch after creation
    #[arg(long)]
    pub launch: Option<String>,

    /// Agent root directory of the caller, accepted for parity with create-agent
    #[arg(long)]
    pub root: Option<String>,

    /// Session token, accepted for parity with create-agent
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentMatrixResult {
    agent_path: String,
    agent_name: String,
    role_path: String,
    launched: bool,
    launch_agent: Option<String>,
}

pub fn execute(args: CreateAgentMatrixArgs) -> i32 {
    let settings = crate::config::settings::load_settings_for_cli();
    let config_dir = crate::config::config_dir();

    let created = match create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &args.project,
        name: &args.name,
        description: &args.description,
        role_template_id: args.role_template.as_deref(),
        settings: &settings,
        config_dir: config_dir.as_deref(),
    }) {
        Ok(created) => created,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let flags = AgentMatrixSettingsFlags::from_settings(&settings);
    for warning in apply_agent_matrix_settings_files(&created.agent_dir, flags) {
        eprintln!("Warning: {}", warning);
    }

    let agent_path = created.agent_dir.to_string_lossy().to_string();
    let role_path = created.role_path.to_string_lossy().to_string();
    let mut launched = false;
    let mut launch_agent_id: Option<String> = None;

    if let Some(ref requested) = args.launch {
        match create_agent::find_launch_agent(&settings, requested) {
            Some(agent) => {
                match create_agent::build_session_request(
                    agent_path.clone(),
                    created.display_name.clone(),
                    agent,
                ) {
                    Ok(request) => match create_agent::write_session_request(&request) {
                        Ok(()) => {
                            launched = true;
                            launch_agent_id = Some(agent.id.clone());
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: agent matrix created but failed to request launch: {}",
                                e
                            );
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "Warning: agent matrix created but failed to request launch: {}",
                            e
                        );
                    }
                }
            }
            None => {
                let available: Vec<&str> = settings.agents.iter().map(|a| a.id.as_str()).collect();
                eprintln!(
                    "Warning: agent '{}' not found in settings. Available: {}. Matrix created but not launched.",
                    requested,
                    available.join(", ")
                );
            }
        }
    }

    let result = CreateAgentMatrixResult {
        agent_path,
        agent_name: created.display_name,
        role_path,
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
