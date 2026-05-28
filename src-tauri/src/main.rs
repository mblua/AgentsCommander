#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{CommandFactory, FromArgMatches};

fn main() {
    // Resolve actual binary name at runtime so --help shows the correct name.
    // Leaked once at startup - lives for the process lifetime.
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
                        ) {
                            std::env::set_var("AC_MACHINE_OUTPUT", "1");
                        }
                        // Install the same logger backend the GUI uses so

                        // every `log::*` call from CLI verbs (the `[task]`
                        // audit lines in particular - plan #137 §3a HIGH-1
                        // mitigation) reaches stderr + <config_dir>/app.log.
                        // GATED on `cli.command.is_some()` so the GUI branch
                        // below initializes via `lib::run()` exactly once.
                        agentscommander_lib::logging::init_logger();
                        let code = agentscommander_lib::cli::handle_cli(cmd);
                        std::process::exit(code);
                    }
                    None => {
                        // GUI mode (with or without --app)
                        if !try_acquire_single_instance() {
                            // Another GUI instance is already running - exit silently
                            std::process::exit(0);
                        }
                        agentscommander_lib::run();
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
            // Failed to create mutex - let it run anyway
            return true;
        }
        // If the mutex already existed, another instance owns it
        GetLastError() != ERROR_ALREADY_EXISTS
    }
    // Note: we intentionally do NOT close the handle - it must stay alive
    // for the lifetime of the process to hold the mutex.
}

#[cfg(not(target_os = "windows"))]
fn try_acquire_single_instance() -> bool {
    true // No single-instance enforcement on non-Windows
}
