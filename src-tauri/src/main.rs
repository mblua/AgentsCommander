#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

fn main() {
    // Try to parse CLI args. If a subcommand is recognized → CLI mode.
    // If no args or unrecognized args (Tauri passes paths etc.) → App mode.
    let args = agentscommander_lib::cli::Cli::try_parse();
    match args {
        Ok(cli) => match cli.command {
            Some(cmd) => {
                let code = agentscommander_lib::cli::handle_cli(cmd);
                std::process::exit(code);
            }
            None => agentscommander_lib::run(),
        },
        Err(_) => agentscommander_lib::run(),
    }
}
