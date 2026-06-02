use clap::{CommandFactory, FromArgMatches};

fn main() {
    let binary_name: &'static str = Box::leak(
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "agentscommander".to_string())
            .into_boxed_str(),
    );

    let cmd = agentscommander_lib::cli::Cli::command().name(binary_name);

    match cmd.try_get_matches() {
        Ok(matches) => match agentscommander_lib::cli::Cli::from_arg_matches(&matches) {
            Ok(cli) => match cli.command {
                Some(cmd) => {
                    if matches!(
                        cmd,
                        agentscommander_lib::cli::Commands::ListPeers(_)
                            | agentscommander_lib::cli::Commands::ListPeersLean(_)
                            | agentscommander_lib::cli::Commands::ListSessions(_)
                    ) {
                        std::env::set_var("AC_MACHINE_OUTPUT", "1");
                    }
                    agentscommander_lib::logging::init_logger();
                    let code = agentscommander_lib::cli::handle_cli(cmd);
                    std::process::exit(code);
                }
                None => {
                    let _ = agentscommander_lib::cli::Cli::command()
                        .name(binary_name)
                        .print_help();
                    agentscommander_lib::cli::flush_outputs();
                    std::process::exit(0);
                }
            },
            Err(e) => {
                let _ = e.print();
                agentscommander_lib::cli::flush_outputs();
                std::process::exit(1);
            }
        },
        Err(e) => {
            let _ = e.print();
            agentscommander_lib::cli::flush_outputs();
            std::process::exit(if e.use_stderr() { 1 } else { 0 });
        }
    }
}
