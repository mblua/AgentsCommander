//! #1157 - `injected-messages` CLI verb. Resets operator-editable injected PTY
//! message templates to the defaults this binary ships.
//!
//! No `--token`: this touches the user-local config directory next to the
//! executable, which any local process can already write (the same boundary
//! argument as `open-project` and `coding-agent`).
//!
//! `--all` is `--id` applied to every known id THROUGH THE SAME surgical
//! writer. It is deliberately not a whole-file rewrite: that would discard the
//! operator's comments, unknown keys, entry order and every other id's edits.

use clap::{Args, Subcommand};

use crate::cli_println;
use crate::config::injected_messages::{
    known_message_ids, reseed, ReseedTarget, INJECTED_MESSAGES_FILENAME,
};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Restore injected PTY message templates to this binary's shipped \
defaults, in place.\n\n\
FILE: <config-dir>/injected-messages.toml, next to the executable. Edit it by \
hand to change what AgentsCommander injects; see injected-messages.default.toml \
for the canonical set.\n\n\
SAFETY: a timestamped .bak- copy is written before anything is overwritten. \
Comments, unknown keys, entry order and untargeted entries are preserved.\n\n\
EXIT CODES: 0 on success, 1 on error.")]
pub struct InjectedMessagesArgs {
    #[command(subcommand)]
    pub cmd: InjectedMessagesCmd,
}

#[derive(Subcommand)]
pub enum InjectedMessagesCmd {
    /// Reset one message, or every message, to the shipped default
    Reseed(ReseedArgs),
}

#[derive(Args)]
pub struct ReseedArgs {
    /// Exact message id to reset (e.g. context-alert)
    #[arg(long, conflicts_with = "all", required_unless_present = "all")]
    pub id: Option<String>,

    /// Reset every known message id
    #[arg(long)]
    pub all: bool,
}

pub fn execute(
    args: InjectedMessagesArgs,
    warnings: &crate::config::injected_messages::MessageWarnings,
) -> i32 {
    match args.cmd {
        InjectedMessagesCmd::Reseed(args) => execute_reseed(args, warnings),
    }
}

fn execute_reseed(
    args: ReseedArgs,
    warnings: &crate::config::injected_messages::MessageWarnings,
) -> i32 {
    let target = if args.all {
        ReseedTarget::All
    } else {
        match args.id {
            Some(id) => ReseedTarget::Id(id),
            None => {
                eprintln!(
                    "Error: exactly one of --id <id> or --all is required. Valid ids: {}",
                    known_message_ids().join(", ")
                );
                return 1;
            }
        }
    };

    let Some(dir) = crate::config::config_dir() else {
        eprintln!("Error: this AgentsCommander instance has no resolvable config directory.");
        return 1;
    };

    match reseed(&dir, target, warnings) {
        Ok(ids) => {
            cli_println!(
                "Reset {} in {}",
                ids.join(", "),
                dir.join(INJECTED_MESSAGES_FILENAME).display()
            );
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests;
