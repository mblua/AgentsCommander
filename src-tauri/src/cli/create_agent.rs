use clap::Args;

use crate::cli::create_agent_matrix;
#[cfg(test)]
use crate::config::instance_artifacts::SESSION_REQUESTS_DIR_NAME;

pub(crate) mod session_request;
#[cfg(test)]
pub(crate) use session_request::{
    build_session_request, find_launch_agent, read_session_request_result,
    wait_for_session_request_result, write_session_request, LaunchOutcome,
};
pub(crate) use session_request::{is_session_request_result_path, write_session_request_result};
pub use session_request::{SessionRequest, SessionRequestResult, SessionRequestResultStatus};

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
    agentscommander create-agent --project <PROJECT> --name <NAME> --description <DESC> \
[--role-template <TEMPLATE_ID>] [--launch <AGENT>]\n\n\
VALIDATION:\n  \
  --project is a registered AC project folder name from settings.projectPaths. \
Paths are not accepted.\n  \
  --name is trimmed before use. It must not be empty after trim, and it must not \
contain path separators (/ or \\) or NUL.\n\n\
OUTPUT:\n  \
  Prints the same JSON as create-agent-matrix: agentPath, agentName, rolePath, launched, \
launchStatus, launchError, launchAgent.")]
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
    /// Must match an agent id or label from settings.json.
    /// If omitted, the Matrix is created but no session is started
    #[arg(long)]
    pub launch: Option<String>,

    /// Agent root directory of the caller (for logging/context)
    #[arg(long)]
    pub root: Option<String>,

    /// Session token (for auth context)
    #[arg(long)]
    pub token: Option<String>,
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

#[cfg(test)]
mod tests;
