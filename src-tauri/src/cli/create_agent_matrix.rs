//! CLI entrypoint for creating Agent Matrices.
//!
//! This command runs out of process, so it reads the persisted settings snapshot
//! with `load_settings_for_cli()` and cannot share the GUI's in-memory
//! `SettingsState`. That matches the existing CLI mutation
//! model: local disk creation is immediate, while launch requests are handed to
//! the running GUI through the session-request mailbox.

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::cli::create_agent;
use crate::commands::entity_creation::{create_agent_matrix_on_disk, CreateAgentMatrixDiskArgs};
use crate::config::instance_artifacts::PROJECT_REFRESH_REQUESTS_DIR_NAME;
use crate::config::projects::resolve_project_reference;

#[derive(Args)]
#[command(after_help = "\
NOTES:\n  \
  --project is a registered AC project folder name from settings.projectPaths. Paths are not accepted.\n  \
  The target project must contain .ac.\n  \
  --name is sanitized by the same backend as the UI into a lower-case Matrix id.\n  \
  --role-template must be an id from the same source as the New Agent picker.\n  \
  Invalid templates fail before creating the target Matrix directory.\n  \
  Output is JSON with agentPath, agentName, rolePath, launched, launchAgent.")]
pub struct CreateAgentMatrixArgs {
    /// Registered AC project folder name. Paths are not accepted.
    #[arg(long, value_name = "PROJECT")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRefreshRequest {
    pub id: String,
    pub project_path: String,
    #[serde(alias = "agentPath")]
    pub changed_path: Option<String>,
    #[serde(alias = "agentName")]
    pub changed_name: Option<String>,
    pub reason: String,
    pub timestamp: String,
}

pub fn execute(args: CreateAgentMatrixArgs) -> i32 {
    execute_matrix_project_create(
        &args.project,
        &args.name,
        &args.description,
        args.role_template.as_deref(),
        args.launch.as_deref(),
    )
}

pub(crate) fn execute_matrix_project_create(
    project: &str,
    name: &str,
    description: &str,
    role_template: Option<&str>,
    launch: Option<&str>,
) -> i32 {
    let settings = crate::config::settings::load_settings_for_cli();
    let config_dir = crate::config::config_dir();
    let resolved_project = match resolve_project_reference(&settings.project_paths, project) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let project_path = resolved_project.path.to_string_lossy().to_string();

    let created = match create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_path,
        name,
        description,
        role_template_id: role_template,
        settings: &settings,
        config_dir: config_dir.as_deref(),
    }) {
        Ok(created) => created,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let agent_path = created.agent_dir.to_string_lossy().to_string();
    let role_path = created.role_path.to_string_lossy().to_string();

    let project_path_for_refresh = std::fs::canonicalize(&resolved_project.path)
        .unwrap_or_else(|_| resolved_project.path.clone())
        .to_string_lossy()
        .to_string();
    let agent_path_for_refresh = std::fs::canonicalize(&created.agent_dir)
        .unwrap_or_else(|_| created.agent_dir.clone())
        .to_string_lossy()
        .to_string();
    let refresh_request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: project_path_for_refresh,
        changed_path: Some(agent_path_for_refresh),
        changed_name: Some(created.display_name.clone()),
        reason: "createAgentMatrix".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&refresh_request) {
        eprintln!(
            "Warning: agent matrix created but failed to request sidebar refresh: {}",
            e
        );
    }

    let mut launched = false;
    let mut launch_agent_id: Option<String> = None;

    if let Some(requested) = launch {
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

pub(crate) fn write_project_refresh_request(request: &ProjectRefreshRequest) -> Result<(), String> {
    let config_dir =
        crate::config::config_dir().ok_or("Cannot determine config directory".to_string())?;
    let requests_dir = config_dir.join(PROJECT_REFRESH_REQUESTS_DIR_NAME);
    std::fs::create_dir_all(&requests_dir)
        .map_err(|e| format!("Failed to create project-refresh-requests dir: {}", e))?;

    let final_path = requests_dir.join(format!("{}.json", request.id));
    let tmp_path = requests_dir.join(format!("{}.json.tmp", request.id));
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize project refresh request: {}", e))?;

    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write project refresh request: {}", e))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize project refresh request: {}", e)
    })?;
    log::info!(
        "[project-refresh-requests] Wrote request: dir='{}' project='{}' reason='{}'",
        requests_dir.display(),
        request.project_path,
        request.reason
    );

    Ok(())
}
