#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
                                | agentscommander_lib::cli::Commands::UiClick(_)
                                | agentscommander_lib::cli::Commands::UiContextClick(_)
                                | agentscommander_lib::cli::Commands::UiHover(_)
                                | agentscommander_lib::cli::Commands::UiSet(_)
                                | agentscommander_lib::cli::Commands::UiType(_)
                                | agentscommander_lib::cli::Commands::UiBackend(_)
                                | agentscommander_lib::cli::Commands::UiWait(_)
                                | agentscommander_lib::cli::Commands::TaskSetTitle(_)
                                | agentscommander_lib::cli::Commands::CodingAgent(_)
                        ) {
                            std::env::set_var("AC_MACHINE_OUTPUT", "1");
                        }
                        #[cfg(target_os = "linux")]
                        if let Err(error) =
                            agentscommander_lib::config::linux_state::prepare_secure_config_root()
                        {
                            report_startup_error(error, false);
                        }

                        // Install the same logger backend the GUI uses so
                        // every `log::*` call from CLI verbs (the `[task]`
                        // audit lines in particular — plan #137 §3a HIGH-1
                        // mitigation) reaches stderr + <config_dir>/app.log.
                        // GATED on `cli.command.is_some()` so the GUI branch
                        // below initializes via `lib::run()` exactly once.
                        if let Err(error) = agentscommander_lib::logging::init_logger() {
                            report_startup_error(error, false);
                        }

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
                            if std::io::stderr().is_terminal() {
                                if let Some(notice) =
                                    agentscommander_lib::update_check::read_cached_notice()
                                {
                                    eprintln!("{}", notice);
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

                        #[cfg(target_os = "linux")]
                        if let Err(error) =
                            agentscommander_lib::config::linux_state::prepare_secure_config_root()
                        {
                            report_startup_error(error, true);
                        }

                        #[cfg(target_os = "linux")]
                        let mut linux_instance_guard =
                            match agentscommander_lib::config::linux_state::acquire_gui_instance() {
                                Ok(
                                    agentscommander_lib::config::linux_state::GuiLockOutcome::Acquired(
                                        guard,
                                    ),
                                ) => guard,
                                Ok(
                                    agentscommander_lib::config::linux_state::GuiLockOutcome::AlreadyRunning,
                                ) => {
                                    handle_already_running(ui_automation_enabled);
                                }
                                Err(error) => report_startup_error(error, true),
                            };

                        #[cfg(target_os = "linux")]
                        if let Err(error) =
                            agentscommander_lib::config::linux_state::prepare_secure_gui_state(
                                &linux_instance_guard,
                            )
                        {
                            let diagnostics = linux_instance_guard.release();
                            report_startup_error(
                                error.with_rollback_diagnostics(diagnostics),
                                true,
                            );
                        }

                        #[cfg(not(target_os = "linux"))]
                        if !try_acquire_single_instance() {
                            handle_already_running(ui_automation_enabled);
                        }

                        if let Err(error) = agentscommander_lib::logging::init_logger() {
                            #[cfg(target_os = "linux")]
                            let error =
                                error.with_rollback_diagnostics(linux_instance_guard.release());
                            report_startup_error(error, true);
                        }

                        let result = agentscommander_lib::run(placement, ui_automation_enabled);

                        #[cfg(target_os = "linux")]
                        let release_diagnostics = linux_instance_guard.release();
                        #[cfg(not(target_os = "linux"))]
                        let release_diagnostics: Vec<String> = Vec::new();

                        match result {
                            Ok(code) => {
                                for diagnostic in release_diagnostics {
                                    log::warn!("[linux-instance-lock] {diagnostic}");
                                }
                                std::process::exit(code);
                            }
                            Err(error) => {
                                report_startup_error(
                                    error.with_rollback_diagnostics(release_diagnostics),
                                    true,
                                );
                            }
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

fn handle_already_running(ui_automation_enabled: bool) -> ! {
    if ui_automation_enabled {
        if agentscommander_lib::testability::ui_automation::existing_enabled_session_for_current_config()
        {
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
    std::process::exit(0);
}

fn report_startup_error(error: agentscommander_lib::errors::StartupError, gui: bool) -> ! {
    agentscommander_lib::cli::attach_parent_console();
    let message = format!("Error: {error}");
    eprintln!("{message}");
    agentscommander_lib::cli::flush_outputs();
    if gui {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Agents Commander startup error")
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    std::process::exit(1);
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

#[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
fn try_acquire_single_instance() -> bool {
    true // No single-instance enforcement on non-Windows
}
