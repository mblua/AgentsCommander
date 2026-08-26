pub mod agency_templates;
pub mod api_client;
pub mod close_session;
pub mod coding_agent;
pub mod create_agent;
pub mod create_agent_matrix;
pub mod harness;
pub mod injected_messages;
pub mod list_peers;
pub mod list_sessions;
pub mod loop_cmd;
pub mod new_project;
pub mod open_project;
pub mod purge_wg;
pub mod raise_hand;
pub mod role_experiment;
pub mod self_clear;
pub mod self_switch;
pub mod send;
pub mod session_safety;
pub mod task_append_body;
pub mod task_ops;
pub mod task_set_title;
pub mod team;
pub mod telegram_send_image;
pub mod terminal_snapshot;
#[cfg(target_os = "windows")]
pub mod window_list;
#[cfg(target_os = "windows")]
pub mod window_screenshot;
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

    /// Test-only GUI placement: virtual-desktop X coordinate in physical pixels
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub window_x: Option<f64>,

    /// Test-only GUI placement: virtual-desktop Y coordinate in physical pixels
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub window_y: Option<f64>,

    /// Test-only GUI placement: window width in physical pixels
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub window_width: Option<f64>,

    /// Test-only GUI placement: window height in physical pixels
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub window_height: Option<f64>,

    /// Test-only GUI placement: maximize after applying the requested rectangle
    #[arg(long, hide = true)]
    pub window_maximized: bool,

    /// Test-only GUI automation bridge opt-in
    #[arg(long, hide = true)]
    pub ui_automation: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Send a message to another agent
    Send(send::SendArgs),
    /// Hand off via SELF-HANDOFF.md, then clear the calling agent's OWN context and resume from it
    /// (two-phase, deferred: clear after 30s sustained idle, then resume-prompt after a fresh 30s)
    #[command(name = "self-handoff-and-clear")]
    SelfClear(self_clear::SelfClearArgs),
    /// Hand off via SELF-HANDOFF.md, switch or hard-reset the caller, then resume from it
    #[command(name = "self-handoff-and-switch")]
    SelfSwitch(self_switch::SelfSwitchArgs),
    /// Show this session's raised-hand communication indicator in the Sidebar coordinator row
    #[command(name = "raise-hand")]
    RaiseHand(raise_hand::RaiseHandArgs),
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
    #[command(hide = true)]
    RoleExperiment(role_experiment::RoleExperimentArgs),
    /// Close all sessions for a target agent (coordinator authorization required)
    CloseSession(close_session::CloseSessionArgs),
    /// Purge every agent in the caller's own workgroup (coordinator-only, fail-closed busy gate)
    #[command(name = "purge-wg")]
    PurgeWg(purge_wg::PurgeWgArgs),
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
    /// Manage Project Loops
    Loop(loop_cmd::LoopArgs),
    /// Execute commands through the policy harness
    Harness(harness::HarnessArgs),
    /// Manage Coding Agent configurations (settings.agents)
    CodingAgent(coding_agent::CodingAgentArgs),
    /// Manage the injected PTY message templates (injected-messages.toml)
    #[command(name = "injected-messages")]
    InjectedMessages(injected_messages::InjectedMessagesArgs),
    /// Manage control-plane API client tokens (mint / revoke / list; host authority required)
    ApiClient(api_client::ApiClientArgs),
    /// Capture an authorized backend terminal snapshot as JSON or deterministic PNG
    #[command(name = "terminal-snapshot")]
    TerminalSnapshot(terminal_snapshot::TerminalSnapshotArgs),
    /// Delete only the disposable testable app state
    #[command(hide = true)]
    TestReset(crate::testability::reset::TestResetArgs),
    /// Inspect running AgentsCommander GUI windows for this binary identity
    #[command(hide = true)]
    WindowInfo(crate::testability::window_info::WindowInfoArgs),
    /// Query a WebView automation target by data-ac-testid
    #[command(hide = true)]
    UiQuery(crate::testability::ui_automation::UiQueryArgs),
    /// Query or scroll a visible xterm through the test-only automation bridge
    #[command(hide = true)]
    UiTerminal(crate::testability::ui_automation::UiTerminalArgs),
    /// Click a WebView automation target by data-ac-testid
    #[command(hide = true)]
    UiClick(crate::testability::ui_automation::UiClickArgs),
    /// Dispatch a WebView contextmenu event on an automation target by data-ac-testid
    #[command(hide = true)]
    UiContextClick(crate::testability::ui_automation::UiContextClickArgs),
    /// Dispatch a WebView pointer hover transition on an automation target by data-ac-testid
    #[command(hide = true)]
    UiHover(crate::testability::ui_automation::UiHoverArgs),
    /// List live native windows as decimal id and title (Windows only)
    #[cfg(target_os = "windows")]
    WindowList(window_list::WindowListArgs),
    /// Capture a live native window to a PNG file (Windows only)
    #[cfg(target_os = "windows")]
    WindowScreenshot(window_screenshot::WindowScreenshotArgs),
    /// Set an input/select WebView automation target by data-ac-testid
    #[command(hide = true)]
    UiSet(crate::testability::ui_automation::UiSetArgs),
    /// Type text into a WebView automation target by data-ac-testid
    #[command(hide = true)]
    UiType(crate::testability::ui_automation::UiTypeArgs),
    /// Execute a Rust-handled automation hook
    #[command(hide = true)]
    UiBackend(crate::testability::ui_automation::UiBackendArgs),
    /// Wait until a WebView automation target is available by data-ac-testid
    #[command(hide = true)]
    UiWait(crate::testability::ui_automation::UiWaitArgs),
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
        Commands::SelfClear(args) => self_clear::execute(args),
        Commands::SelfSwitch(args) => self_switch::execute(args),
        Commands::RaiseHand(args) => raise_hand::execute(args),
        Commands::ListPeers(args) => list_peers::execute(args),
        Commands::ListPeersLean(args) => list_peers::execute_lean(args),
        Commands::ListSessions(args) => list_sessions::execute(args),
        Commands::AgencyTemplates(args) => agency_templates::execute(args),
        Commands::CreateAgent(args) => create_agent::execute(args),
        Commands::CreateAgentMatrix(args) => create_agent_matrix::execute(args),
        Commands::RoleExperiment(args) => role_experiment::execute(args),
        Commands::CloseSession(args) => close_session::execute(args),
        Commands::PurgeWg(args) => purge_wg::execute(args),
        Commands::TaskSetTitle(args) => task_set_title::execute(args),
        Commands::TaskAppendBody(args) => task_append_body::execute(args),
        Commands::OpenProject(args) => open_project::execute(args),
        Commands::NewProject(args) => new_project::execute(args),
        Commands::TelegramSendImage(args) => telegram_send_image::execute(args),
        Commands::Workgroup(args) => workgroup::execute(args),
        Commands::Team(args) => team::execute(args),
        Commands::Loop(args) => loop_cmd::execute(args),
        Commands::Harness(args) => harness::execute(args),
        Commands::CodingAgent(args) => coding_agent::execute(args),
        Commands::InjectedMessages(args) => injected_messages::execute(args),
        Commands::ApiClient(args) => api_client::execute(args),
        Commands::TerminalSnapshot(args) => terminal_snapshot::execute(args),
        Commands::TestReset(args) => crate::testability::reset::execute(args),
        Commands::WindowInfo(args) => crate::testability::window_info::execute(args),
        Commands::UiQuery(args) => crate::testability::ui_automation::execute_query(args),
        Commands::UiTerminal(args) => crate::testability::ui_automation::execute_terminal(args),
        Commands::UiClick(args) => crate::testability::ui_automation::execute_click(args),
        Commands::UiContextClick(args) => {
            crate::testability::ui_automation::execute_context_click(args)
        }
        Commands::UiHover(args) => crate::testability::ui_automation::execute_hover(args),
        #[cfg(target_os = "windows")]
        Commands::WindowList(args) => window_list::execute(args),
        #[cfg(target_os = "windows")]
        Commands::WindowScreenshot(args) => window_screenshot::execute(args),
        Commands::UiSet(args) => crate::testability::ui_automation::execute_set(args),
        Commands::UiType(args) => crate::testability::ui_automation::execute_type(args),
        Commands::UiBackend(args) => crate::testability::ui_automation::execute_backend(args),
        Commands::UiWait(args) => crate::testability::ui_automation::execute_wait(args),
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

    /// #654: internal `role-experiment` + the test/automation verbs and the
    /// test-only top-level flags must NOT clutter `--help`. `hide = true` is a
    /// display-only attribute, so each must remain a registered subcommand/arg
    /// (the test harness still invokes them by name).
    #[test]
    fn internal_verbs_and_test_flags_are_hidden_from_help() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();

        let hidden_verbs = [
            "role-experiment",
            "test-reset",
            "window-info",
            "ui-query",
            "ui-terminal",
            "ui-click",
            "ui-context-click",
            "ui-hover",
            "ui-set",
            "ui-type",
            "ui-backend",
            "ui-wait",
        ];
        for name in hidden_verbs {
            assert!(
                !help.contains(name),
                "hidden verb `{}` must not appear in --help:\n{}",
                name,
                help
            );
            let sub = cmd
                .get_subcommands()
                .find(|c| c.get_name() == name)
                .unwrap_or_else(|| panic!("`{}` must still be a registered subcommand", name));
            assert!(sub.is_hide_set(), "`{}` should be marked hidden", name);
        }

        // #944 - the array above is a manual allowlist. A new ui-* verb that forgets
        // BOTH `#[command(hide = true)]` and its entry here would ship visible in
        // `--help` with no test failing. Make the invariant structural: every ui-*
        // subcommand is internal, no exceptions, no maintenance.
        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            if name.starts_with("ui-") {
                assert!(
                    sub.is_hide_set(),
                    "`{name}` is a ui-* automation verb and must be hidden from --help"
                );
            }
        }

        for flag in [
            "--window-x",
            "--window-y",
            "--window-width",
            "--window-height",
            "--window-maximized",
            "--ui-automation",
        ] {
            assert!(
                !help.contains(flag),
                "hidden flag `{}` must not appear in --help:\n{}",
                flag,
                help
            );
        }

        // Sanity: a normal public verb is still listed.
        assert!(
            help.contains("send"),
            "public verbs should still appear in --help:\n{}",
            help
        );
    }

    #[test]
    fn hidden_internal_verbs_still_parse_by_name() {
        use clap::Parser;
        let parsed = Cli::try_parse_from(["agentscommander", "window-info"])
            .expect("hidden `window-info` verb must still parse by name");
        assert!(
            matches!(parsed.command, Some(Commands::WindowInfo(_))),
            "expected WindowInfo subcommand"
        );

        let parsed = Cli::try_parse_from([
            "agentscommander",
            "ui-terminal",
            "--session-id",
            "session-a",
            "top",
        ])
        .expect("hidden `ui-terminal` verb must still parse by name");
        assert!(
            matches!(parsed.command, Some(Commands::UiTerminal(_))),
            "expected UiTerminal subcommand"
        );
    }

    #[test]
    fn hidden_test_only_flags_still_parse() {
        use clap::Parser;
        let parsed = Cli::try_parse_from([
            "agentscommander",
            "--window-x",
            "100",
            "--window-y",
            "200",
            "--window-width",
            "800",
            "--window-height",
            "600",
            "--window-maximized",
            "--ui-automation",
        ])
        .expect("hidden test-only flags must still parse");
        assert_eq!(parsed.window_x, Some(100.0));
        assert_eq!(parsed.window_y, Some(200.0));
        assert_eq!(parsed.window_width, Some(800.0));
        assert_eq!(parsed.window_height, Some(600.0));
        assert!(parsed.window_maximized);
        assert!(parsed.ui_automation);
        assert!(parsed.command.is_none());
    }
}
