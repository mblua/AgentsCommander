pub mod create_agent;
pub mod send;
pub mod list_peers;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agentscommander")]
#[command(about = "Agent terminal session manager with inter-agent messaging")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Send a message to another agent
    Send(send::SendArgs),
    /// List available peers for messaging
    ListPeers(list_peers::ListPeersArgs),
    /// Create a new agent: folder + CLAUDE.md, optionally launch it
    CreateAgent(create_agent::CreateAgentArgs),
}

/// Attach to parent console on Windows release builds so CLI output is visible.
#[cfg(target_os = "windows")]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
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
