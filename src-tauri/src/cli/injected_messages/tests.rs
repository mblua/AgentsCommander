use super::*;
use crate::cli::Cli;
use clap::{CommandFactory, Parser};

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(args)
}

#[test]
fn reseed_requires_exactly_one_of_id_or_all() {
    Cli::command().debug_assert();

    assert!(parse(&["ac", "injected-messages", "reseed", "--id", "context-alert"]).is_ok());
    assert!(parse(&["ac", "injected-messages", "reseed", "--all"]).is_ok());
    // Neither, and both, are rejected at the argument boundary.
    assert!(parse(&["ac", "injected-messages", "reseed"]).is_err());
    assert!(parse(&[
        "ac",
        "injected-messages",
        "reseed",
        "--all",
        "--id",
        "context-alert"
    ])
    .is_err());
}

#[test]
fn verb_is_documented_in_help() {
    let help = Cli::command().render_long_help().to_string();
    assert!(
        help.contains("injected-messages"),
        "the verb must be discoverable in --help:\n{}",
        help
    );
}
