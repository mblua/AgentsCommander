pub mod create_agent;
pub mod send;
pub mod list_peers;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agentscommander")]
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

/// Dispatch CLI subcommands. Returns exit code.
pub fn handle_cli(cmd: Commands) -> i32 {
    attach_parent_console();

    match cmd {
        Commands::Send(args) => send::execute(args),
        Commands::ListPeers(args) => list_peers::execute(args),
        Commands::CreateAgent(args) => create_agent::execute(args),
    }
}
