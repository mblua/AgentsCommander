#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{CommandFactory, Parser};

fn main() {
    let args = agentscommander_lib::cli::Cli::try_parse();
    match args {
        Ok(cli) => match cli.command {
            Some(_) if cli.app => {
                agentscommander_lib::cli::attach_parent_console();
                eprintln!("error: --app cannot be combined with a subcommand");
                std::process::exit(1);
            }
            Some(cmd) => {
                let code = agentscommander_lib::cli::handle_cli(cmd);
                std::process::exit(code);
            }
            None if cli.app => agentscommander_lib::run(),
            None => {
                // No subcommand and no --app: print help and exit
                agentscommander_lib::cli::attach_parent_console();
                let _ = agentscommander_lib::cli::Cli::command().print_help();
                std::process::exit(2);
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
