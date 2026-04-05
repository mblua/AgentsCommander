pub mod create_agent;
pub mod list_peers;
pub mod list_sessions;
pub mod send;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Agent terminal session manager with inter-agent messaging")]
#[command(after_help = "\
TOKEN: Your session token is injected into your console as a '# === Session Credentials ===' block \
when your session starts. If it expires, any failed `send` triggers an automatic token refresh.\n\n\
EXIT CODES: All subcommands return 0 on success, 1 on error.\n\n\
AGENT NAMES: Agents are identified by their path-based name (e.g., \"repos/my-project\"). \
Use `list-peers` to discover valid agent names before sending messages.")]
pub struct Cli {
    /// Launch the GUI application
    #[arg(long)]
    pub app: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Send a message to another agent
    Send(send::SendArgs),
    /// List reachable peers (returns JSON array with name, status, role, teams)
    ListPeers(list_peers::ListPeersArgs),
    /// List all sessions in the running app instance (returns JSON)
    ListSessions(list_sessions::ListSessionsArgs),
    /// Create a new agent: folder + CLAUDE.md, optionally launch it
    CreateAgent(create_agent::CreateAgentArgs),
}

/// Attach to parent console on Windows release builds so CLI output is visible.
#[cfg(target_os = "windows")]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, AllocConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn attach_parent_console() {
    // No-op on non-Windows
}

/// Validate CLI token: must be provided and must be either the root_token or a valid UUID.
/// Returns `Ok((token_string, is_root))` on success, or an error message on failure.
/// `is_root` is true when the token matches the persisted root_token in settings.
pub fn validate_cli_token(token: &Option<String>) -> Result<(String, bool), String> {
    let token = match token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return Err(
                "Error: --token is required. Your session token is in the \
                 '# === Session Credentials ===' block.\n\
                 If you don't have one, output %%ACRC%% to request fresh credentials."
                    .to_string(),
            );
        }
    };

    // Accept root_token from settings
    let settings = crate::config::settings::load_settings();
    if settings.root_token.as_deref() == Some(&token) {
        return Ok((token, true));
    }

    // Otherwise must be a valid UUID (all session tokens are UUIDs)
    if uuid::Uuid::parse_str(&token).is_err() {
        let display = if token.len() > 8 { &token[..8] } else { &token };
        return Err(format!(
            "Error: invalid token '{}...'. Expected a valid session token (UUID) or root token.\n\
             Output %%ACRC%% to request fresh credentials.",
            display
        ));
    }

    Ok((token, false))
}

/// Dispatch CLI subcommands. Returns exit code.
pub fn handle_cli(cmd: Commands) -> i32 {
    attach_parent_console();

    match cmd {
        Commands::Send(args) => send::execute(args),
        Commands::ListPeers(args) => list_peers::execute(args),
        Commands::ListSessions(args) => list_sessions::execute(args),
        Commands::CreateAgent(args) => create_agent::execute(args),
    }
}
