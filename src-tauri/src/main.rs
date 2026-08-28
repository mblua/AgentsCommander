#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use agentscommander_lib::cli::Commands;
use clap::{CommandFactory, FromArgMatches};

fn main() {
    // Resolve actual binary name at runtime so --help shows the correct name.
    // Leaked once at startup — lives for the process lifetime.
    let binary_name: &'static str = Box::leak(
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "agentscommander".to_string())
            .into_boxed_str(),
    );

    let cmd = agentscommander_lib::cli::Cli::command().name(binary_name);

    match cmd.try_get_matches() {
        Ok(matches) => {
            match agentscommander_lib::cli::Cli::from_arg_matches(&matches) {
                Ok(cli) => match cli.command {
                    Some(cmd) => {
                        // Attach to the parent console BEFORE init_logger so
                        // any startup eprintln! (e.g. the "[log] file logging
                        // to ..." line) reaches the user's terminal on
                        // Windows release builds (where the binary is linked
                        // with `windows_subsystem = "windows"` and starts
                        // with no attached stderr).

                        agentscommander_lib::cli::attach_parent_console();
                        if matches!(
                            cmd,
                            agentscommander_lib::cli::Commands::ListPeers(_)
                                | agentscommander_lib::cli::Commands::ListPeersLean(_)
                                | agentscommander_lib::cli::Commands::ListSessions(_)
                                | agentscommander_lib::cli::Commands::AgencyTemplates(_)
                                | agentscommander_lib::cli::Commands::UiQuery(_)
                                | agentscommander_lib::cli::Commands::UiTerminal(_)
                                | agentscommander_lib::cli::Commands::UiClick(_)
                                | agentscommander_lib::cli::Commands::UiContextClick(_)
                                | agentscommander_lib::cli::Commands::UiHover(_)
                                | agentscommander_lib::cli::Commands::UiSet(_)
                                | agentscommander_lib::cli::Commands::UiType(_)
                                | agentscommander_lib::cli::Commands::UiBackend(_)
                                | agentscommander_lib::cli::Commands::UiWait(_)
                                | agentscommander_lib::cli::Commands::TaskSetTitle(_)
                                | agentscommander_lib::cli::Commands::CodingAgent(_)
                                | agentscommander_lib::cli::Commands::TerminalSnapshot(_)
                        ) {
                            std::env::set_var("AC_MACHINE_OUTPUT", "1");
                        }
                        if !matches!(&cmd, Commands::TestReset(_)) {
                            if let Err(error) = agentscommander_lib::preflight_config_startup() {
                                agentscommander_lib::cli::present_fatal_startup_message(
                                    &error.to_string(),
                                );
                                std::process::exit(1);
                            }
                            // Install the same logger backend the GUI uses so

                            // every `log::*` call from CLI verbs (the `[task]`
                            // audit lines in particular — plan #137 §3a HIGH-1
                            // mitigation) reaches stderr + <config_dir>/app.log.
                            // GATED on `cli.command.is_some()` so the GUI branch
                            // below initializes via `lib::run()` exactly once.
                            agentscommander_lib::logging::init_logger();

                            // Issue #609 Phase 2 - one-line "update available" notice for
                            // terminal runs. Cache-only (no network, no blocking). M1: gate
                            // on an INTERACTIVE stderr, not the AC_MACHINE_OUTPUT allowlist -
                            // that allowlist (above) covers only ListPeers/ListPeersLean/
                            // ListSessions/AgencyTemplates/Ui*, so `send`, `task`,
                            // `window-info`, and any future machine verb would get stderr
                            // spam for up to 24h while an update is pending. `is_terminal()`
                            // is the future-proof gate: a human at a terminal sees the
                            // notice; agents, scripts, and piped/redirected runs (stderr is
                            // not a tty) stay silent.
                            {
                                use std::io::IsTerminal;
                                if std::io::stderr().is_terminal()
                                    && std::env::var_os("AC_MACHINE_OUTPUT").is_none()
                                {
                                    if let Some(notice) =
                                        agentscommander_lib::update_check::read_cached_notice()
                                    {
                                        eprintln!("{}", notice);
                                    }
                                }
                            }
                        }

                        let code = agentscommander_lib::cli::handle_cli(cmd);
                        std::process::exit(code);
                    }
                    None => {
                        // GUI mode (with or without --app)
                        let placement = match agentscommander_lib::testability::window_placement::resolve_from_cli_or_env(
                            cli.window_x,
                            cli.window_y,
                            cli.window_width,
                            cli.window_height,
                            cli.window_maximized,
                        ) {
                            Ok(placement) => placement,
                            Err(e) => {
                                agentscommander_lib::cli::attach_parent_console();
                                eprintln!("Error: {}", e);
                                agentscommander_lib::cli::flush_outputs();
                                std::process::exit(1);
                            }
                        };

                        let ui_automation_enabled = match agentscommander_lib::testability::ui_automation::resolve_enabled_from_cli_or_env(cli.ui_automation) {
                            Ok(enabled) => enabled,
                            Err(e) => {
                                agentscommander_lib::cli::attach_parent_console();
                                eprintln!("{}", e);
                                agentscommander_lib::cli::flush_outputs();
                                std::process::exit(1);
                            }
                        };

                        if !try_acquire_single_instance() {
                            if ui_automation_enabled {
                                if agentscommander_lib::testability::ui_automation::existing_enabled_session_for_current_config() {
                                    std::process::exit(0);
                                }
                                agentscommander_lib::cli::attach_parent_console();
                                eprintln!(
                                    "{}",
                                    agentscommander_lib::testability::ui_automation::automation_not_enabled_json()
                                );
                                agentscommander_lib::cli::flush_outputs();
                                std::process::exit(1);
                            }
                            agentscommander_lib::cli::present_fatal_startup_message(
                                "An AgentsCommander instance with this executable identity is already running.\n\nRename this executable to agentscommander_<name>.exe to start an independent instance with its own configuration directory and ports.",
                            );
                            std::process::exit(0);
                        }
                        if let Err(error) =
                            agentscommander_lib::run(placement, ui_automation_enabled)
                        {
                            agentscommander_lib::cli::present_fatal_startup_message(
                                &error.to_string(),
                            );
                            std::process::exit(1);
                        }
                    }
                },
                Err(e) => {
                    agentscommander_lib::cli::attach_parent_console();
                    let _ = e.print();
                    agentscommander_lib::cli::flush_outputs();
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            // --help, --version, or invalid args: print and exit
            agentscommander_lib::cli::attach_parent_console();
            let _ = e.print();
            agentscommander_lib::cli::flush_outputs();
            std::process::exit(if e.use_stderr() { 1 } else { 0 });
        }
    }
}

/// Try to acquire a system-wide named mutex.
/// Returns true if this is the first GUI instance, false if one is already running.
#[cfg(target_os = "windows")]
fn try_acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    const ERROR_ALREADY_EXISTS: u32 = 183;

    let mutex_name = agentscommander_lib::config::profile::mutex_name();
    let name: Vec<u16> = mutex_name.encode_utf16().collect();

    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // Failed to create mutex — let it run anyway
            return true;
        }
        // If the mutex already existed, another instance owns it
        GetLastError() != ERROR_ALREADY_EXISTS
    }
    // Note: we intentionally do NOT close the handle — it must stay alive
    // for the lifetime of the process to hold the mutex.
}

#[cfg(not(target_os = "windows"))]
fn try_acquire_single_instance() -> bool {
    true // No single-instance enforcement on non-Windows
}
