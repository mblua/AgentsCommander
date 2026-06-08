pub mod agency_templates;
pub mod close_session;
pub mod create_agent;
pub mod create_agent_matrix;
pub mod harness;
pub mod list_peers;
pub mod list_sessions;
pub mod new_project;
pub mod open_project;
pub mod role_experiment;
pub mod send;
pub mod session_safety;
pub mod task_append_body;
pub mod task_ops;
pub mod task_set_title;
pub mod team;
pub mod telegram_send_image;
pub mod workgroup;

use clap::{Parser, Subcommand};

/// Print to stdout with BrokenPipe tolerance. If the underlying write fails
/// SPECIFICALLY with `io::ErrorKind::BrokenPipe` (typically because the
/// consumer closed stdout early — `head`, captured pipe in PS without
/// `Out-String`, etc.), exit the process cleanly with code 0 instead of
/// panicking. ANY OTHER write error (`PermissionDenied`, `StorageFull`,
/// `WriteZero`, `Interrupted`, `Other`, etc.) panics like `println!` does
/// today — those are real failures and a silent exit-0 would let consumers
/// believe an empty redirect target is a successful empty result. See #230.
///
/// Use this in CLI verbs anywhere we currently call `println!` for primary
/// output (JSON results, status lines, response bodies). Continue using
/// `eprintln!` for error / log output — stderr is rarely piped and the
/// panic surface there is acceptable for now.
#[macro_export]
macro_rules! cli_println {
    () => {{
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut h = stdout.lock();
        if let Err(e) = writeln!(h) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            } else {
                panic!("failed printing to stdout: {}", e);
            }
        }
    }};
    ($($arg:tt)*) => {{
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut h = stdout.lock();
        if let Err(e) = writeln!(h, $($arg)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            } else {
                panic!("failed printing to stdout: {}", e);
            }
        }
    }};
}

#[derive(Parser)]
#[command(about = "Agent terminal session manager with inter-agent messaging")]
#[command(after_help = "\
TOKEN: In agent sessions, pass AGENTSCOMMANDER_TOKEN from the environment. \
Credentials are not delivered through visible PTY text. \
If token validation fails, restart or respawn the sender session; live token refresh is not supported.\n\n\
TOKEN VALIDATION MODEL: The CLI validates token SHAPE only (root token, master token, \
or any valid UUID). Per-session tokens are authoritative at the daemon mailbox boundary, \
not in the CLI. A UUID issued by a different binary instance will pass the CLI's shape \
check but will be REJECTED by the daemon's mailbox (write-path verbs like `send`, \
`close-session`, `task-set-title`, `task-append-body`). Read-only verb `list-peers` \
REQUIRES a token but accepts ANY UUID-shaped value at the CLI boundary — the token value \
is not consulted for authorization at the CLI. It reads disk state directly (from \
`--root` and the binary's per-binary config directory); authorization of any specific \
UUID happens only at the daemon mailbox, which write-path verbs invoke. `list-sessions` \
does not take a token; it reads `sessions.json` from the binary's config directory \
directly and never contacts the daemon mailbox.\n\n\
EXIT CODES: All subcommands return 0 on success, 1 on error.\n\n\
AGENT NAMES: Agents are identified by their path-based name (e.g., \"repos/my-project\"). \
Use `list-peers-lean` to discover valid agent names before sending messages.")]
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
    /// List reachable peers (lean JSON — name, working, sessionStatus, reachable, teams)
    ListPeersLean(list_peers::ListPeersLeanArgs),
    /// List all sessions in the running app instance (returns JSON)
    ListSessions(list_sessions::ListSessionsArgs),
    /// Manage downloaded Agency Agents role templates
    AgencyTemplates(agency_templates::AgencyTemplatesArgs),
    /// Create a new Agent Matrix in a registered project
    CreateAgent(create_agent::CreateAgentArgs),
    /// Create a full Agent Matrix in an AC project, optionally from a role template
    CreateAgentMatrix(create_agent_matrix::CreateAgentMatrixArgs),
    /// Manage Role.md variant experiments
    RoleExperiment(role_experiment::RoleExperimentArgs),
    /// Close all sessions for a target agent (coordinator authorization required)
    CloseSession(close_session::CloseSessionArgs),
    /// Set the title field in the workgroup TASK.md frontmatter (coordinator-only)
    TaskSetTitle(task_set_title::TaskSetTitleArgs),
    /// Append text to the body of the workgroup TASK.md (coordinator-only)
    TaskAppendBody(task_append_body::TaskAppendBodyArgs),
    /// Register an existing AC project (.ac must already exist) in settings
    OpenProject(open_project::OpenProjectArgs),
    /// Create an AC project (create `.ac/` Project AC Root if missing) and register it in settings
    NewProject(new_project::NewProjectArgs),
    /// Send a local image/file to a configured Telegram bot (no GUI required)
    TelegramSendImage(telegram_send_image::TelegramSendImageArgs),
    /// Manage workgroups in an AC project
    Workgroup(workgroup::WorkgroupArgs),
    /// Manage teams and scoped workgroup membership
    Team(team::TeamArgs),
    /// Execute commands through the policy harness
    Harness(harness::HarnessArgs),
}

/// Attach to parent console (or allocate a new one) ONLY if both stdout and stderr
/// have invalid/missing handles. When stdio is already valid (inherited pipes,
/// inherited console handles, or file redirects from the parent), `AttachConsole`
/// would REBIND the std handles to a fresh console buffer — breaking those
/// inherited channels. That rebinding is the root cause of issue #129: PS
/// -NonInteractive `&` direct calls inherit pipe handles to the GUI-subsystem
/// child, and AttachConsole's rebind sends subsequent writes to a console buffer
/// that PS does not surface, dropping all output.
///
/// The condition uses `GetFileType` on the std handles. `FILE_TYPE_UNKNOWN`
/// (returned for null/invalid handles) is the only case where attaching is
/// useful (the user double-clicked the GUI exe in explorer.exe, etc.). For PIPE,
/// CHAR, DISK, REMOTE — the inherited handle is already routable, leave it alone.
#[cfg(target_os = "windows")]
#[allow(clippy::collapsible_if)]
pub fn attach_parent_console() {
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN};
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let err = GetStdHandle(STD_ERROR_HANDLE);

        // GetFileType returns FILE_TYPE_UNKNOWN for null/invalid handles. Short-
        // circuit the null check first so GetFileType is never called on null.
        let out_invalid = out.is_null() || GetFileType(out) == FILE_TYPE_UNKNOWN;
        let err_invalid = err.is_null() || GetFileType(err) == FILE_TYPE_UNKNOWN;

        if out_invalid && err_invalid {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn attach_parent_console() {
    // No-op on non-Windows
}

/// CLI-side SHAPE validation for a session token. Returns `Ok((token_string, is_root))`
/// on success, or an error message on failure. `is_root` is true when the token matches
/// the persisted `settings.root_token` or `master-token.txt`.
///
/// This is NOT an authorization check. It only ensures the token exists and is parseable
/// as a UUID (or matches the binary's root/master token). The real per-session token check
/// happens at the daemon mailbox boundary (`phone::mailbox::process_message`), which
/// calls `SessionManager::find_by_token` against the live in-memory session set.
///
/// Consequence: any valid UUID-shaped string passes this check. A UUID issued by a
/// different binary instance will pass here but be rejected by the daemon's mailbox.
/// Documented in `cli::Cli`'s `after_help` under TOKEN VALIDATION MODEL. See #229.
pub fn validate_cli_token(token: &Option<String>) -> Result<(String, bool), String> {
    let token = match token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return Err(
                "Error: --token is required. In agent sessions, pass AGENTSCOMMANDER_TOKEN \
                 from the environment. If it is unavailable, restart or respawn the session."
                    .to_string(),
            );
        }
    };

    // Accept root_token from settings
    let settings = crate::config::settings::load_settings();
    if settings.root_token.as_deref() == Some(&token) {
        return Ok((token, true));
    }

    // Accept master token from persisted file
    if let Some(master_path) = crate::config::config_dir().map(|d| d.join("master-token.txt")) {
        if let Ok(master) = std::fs::read_to_string(&master_path) {
            if master.trim() == token {
                return Ok((token, true));
            }
        }
    }

    // Otherwise must be a valid UUID (all session tokens are UUIDs)
    if uuid::Uuid::parse_str(&token).is_err() {
        return Err(
            "Error: invalid token supplied. Expected a valid session token (UUID) or root token. \
             In agent sessions, use AGENTSCOMMANDER_TOKEN from the environment. If validation \
             keeps failing, restart or respawn the session."
                .to_string(),
        );
    }

    Ok((token, false))
}

/// Dispatch CLI subcommands. Returns exit code.
///
/// Caller contract: `attach_parent_console()` MUST be called before this — see
/// `main.rs`. Done there (not here) so the eprintln!s inside `init_logger()`
/// reach the user's terminal on Windows release builds.
pub fn handle_cli(cmd: Commands) -> i32 {
    let code = match cmd {
        Commands::Send(args) => send::execute(args),
        Commands::ListPeers(args) => list_peers::execute(args),
        Commands::ListPeersLean(args) => list_peers::execute_lean(args),
        Commands::ListSessions(args) => list_sessions::execute(args),
        Commands::AgencyTemplates(args) => agency_templates::execute(args),
        Commands::CreateAgent(args) => create_agent::execute(args),
        Commands::CreateAgentMatrix(args) => create_agent_matrix::execute(args),
        Commands::RoleExperiment(args) => role_experiment::execute(args),
        Commands::CloseSession(args) => close_session::execute(args),
        Commands::TaskSetTitle(args) => task_set_title::execute(args),
        Commands::TaskAppendBody(args) => task_append_body::execute(args),
        Commands::OpenProject(args) => open_project::execute(args),
        Commands::NewProject(args) => new_project::execute(args),
        Commands::TelegramSendImage(args) => telegram_send_image::execute(args),
        Commands::Workgroup(args) => workgroup::execute(args),
        Commands::Team(args) => team::execute(args),
        Commands::Harness(args) => harness::execute(args),
    };

    flush_outputs();
    code
}

/// Flush stdout and stderr. Called before any `std::process::exit` to ensure
/// that pending writes are committed before the process is torn down.
///
/// `std::process::exit` skips destructors, so the default flush-on-drop
/// behavior of `Stdout`/`Stderr` does not run. This helper forces an
/// explicit flush. Errors are silenced — there is nothing meaningful to do
/// with a flush failure at process exit.
pub fn flush_outputs() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_cli_token_does_not_echo_invalid_input() {
        let supplied = "super-secret-token-with-hidden-garbage";
        let err = validate_cli_token(&Some(supplied.to_string())).unwrap_err();

        assert!(err.contains("invalid token supplied"));
        assert!(!err.contains(supplied));
        assert!(!err.contains(&supplied[..8]));
        let legacy_phrase = ["Session", "Credentials"].join(" ");
        assert!(!err.contains(&legacy_phrase));
        assert!(!err.to_ascii_lowercase().contains("fallback"));
        assert!(!err.to_ascii_lowercase().contains("visible"));
    }

    #[test]
    fn validate_cli_token_missing_token_documents_env_only() {
        let err = validate_cli_token(&None).unwrap_err();

        assert!(err.contains("AGENTSCOMMANDER_TOKEN"));
        assert!(err.contains("restart or respawn"));
        let legacy_phrase = ["Session", "Credentials"].join(" ");
        assert!(!err.contains(&legacy_phrase));
        assert!(!err.to_ascii_lowercase().contains("fallback"));
        assert!(!err.to_ascii_lowercase().contains("visible"));
    }

    #[test]
    fn cli_after_help_documents_token_validation_model() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let after = cmd
            .get_after_help()
            .expect("Cli after_help should be present")
            .to_string();
        assert!(
            after.contains("TOKEN VALIDATION MODEL"),
            "after_help missing TOKEN VALIDATION MODEL block: {}",
            after
        );
    }
}
