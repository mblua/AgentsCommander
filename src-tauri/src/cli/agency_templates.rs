use clap::{Args, Subcommand};
use serde::Serialize;

use crate::config::agency_cache::{config_dir_or_err, list_cache_at};
pub use crate::config::agency_cache::{
    status_cache, status_cache_at, update_cache, update_cache_at, AgencyTemplatesUpdateArgs,
    UpdateResult, DEFAULT_REFERENCE, DEFAULT_REPO,
};

#[derive(Args)]
#[command(after_help = "\
NOTES:\n  \
  Downloads never run during app startup, list, resolve, or create.\n  \
  update writes <config-dir>/agency-agents_templates atomically.\n  \
  list and status read only the validated local cache and print JSON.")]
pub struct AgencyTemplatesArgs {
    #[command(subcommand)]
    pub command: AgencyTemplatesCommand,
}

#[derive(Subcommand)]
pub enum AgencyTemplatesCommand {
    /// Download or refresh the Agency Agents template cache
    Update(AgencyTemplatesUpdateArgs),
    /// Print cached Agency template metadata as JSON
    List(AgencyTemplatesListArgs),
    /// Print cache manifest/status as JSON
    Status(AgencyTemplatesStatusArgs),
}

#[derive(Args)]
pub struct AgencyTemplatesListArgs {
    /// Compatibility no-op. agency-templates list always prints JSON.
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args)]
pub struct AgencyTemplatesStatusArgs {
    /// Compatibility no-op. agency-templates status always prints JSON.
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

pub fn execute(args: AgencyTemplatesArgs) -> i32 {
    let result = match args.command {
        AgencyTemplatesCommand::Update(args) => update(args),
        AgencyTemplatesCommand::List(args) => list(args),
        AgencyTemplatesCommand::Status(args) => status(args),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> Result<(), String> {
    let json = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}

fn status(args: AgencyTemplatesStatusArgs) -> Result<(), String> {
    let _ = args.json;
    print_json(&status_cache()?, args.pretty)
}

fn list(args: AgencyTemplatesListArgs) -> Result<(), String> {
    let _ = args.json;
    let config_dir = config_dir_or_err()?;
    print_json(&list_cache_at(&config_dir)?, args.pretty)
}

fn update(args: AgencyTemplatesUpdateArgs) -> Result<(), String> {
    let result = update_cache(args)?;
    print_json(&result, false)
}
