#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

fn main() {
    let args = agentscommander_lib::cli::Cli::try_parse();
    match args {
        Ok(cli) => match cli.command {
            Some(cmd) => {
                let code = agentscommander_lib::cli::handle_cli(cmd);
                std::process::exit(code);
            }
            None => {
                // GUI mode (with or without --app)
                if !try_acquire_single_instance() {
                    // Another GUI instance is already running — exit silently
                    std::process::exit(0);
                }
                agentscommander_lib::run();
            }
        },
        Err(e) => {
            // --help, --version, or invalid args: print and exit
            agentscommander_lib::cli::attach_parent_console();
            let _ = e.print();
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

    let name: Vec<u16> = "Local\\AgentsCommander_SingleInstance\0"
        .encode_utf16()
        .collect();

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
