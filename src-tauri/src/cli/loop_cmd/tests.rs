use super::*;
use clap::Parser;

/// The arg structs are private to this file, so parsing goes through the
/// crate's public `Cli` and is then destructured here. Precedent:
/// `cli/mod.rs`'s `hidden_internal_verbs_still_parse_by_name`.
fn create_args(extra: &[&str]) -> LoopCreateArgs {
    let mut argv = vec![
        "agentscommander",
        "loop",
        "create",
        "--project",
        "ProjectAlpha",
        "--name",
        "Daily Standup",
        "--cron",
        "0 9 * * *",
        "--room",
        "wg-1-dev-team",
        "--prompt",
        "Summarize status",
    ];
    argv.extend_from_slice(extra);
    let parsed = crate::cli::Cli::try_parse_from(argv).expect("loop create must parse");
    match parsed.command {
        Some(crate::cli::Commands::Loop(LoopArgs {
            command: LoopCommand::Create(args),
        })) => args,
        _ => panic!("expected `loop create`"),
    }
}

/// AC-1 - an explicit value parses and converts.
#[test]
fn session_start_accumulate_parses_and_converts() {
    let args = create_args(&["--session-start", "accumulate"]);
    assert_eq!(args.session_start, Some(SessionStartCli::Accumulate));
    assert_eq!(
        args.session_start.map(LoopSessionStart::from),
        Some(LoopSessionStart::Accumulate)
    );

    let args = create_args(&["--session-start", "fresh"]);
    assert_eq!(args.session_start, Some(SessionStartCli::Fresh));
    assert_eq!(
        args.session_start.map(LoopSessionStart::from),
        Some(LoopSessionStart::Fresh)
    );
}

/// AC-2 - the flag omitted parses as `None`. `create` turns that into
/// `LoopPolicy::default()`; `update` leaves the stored value alone.
#[test]
fn session_start_absent_parses_as_none() {
    assert_eq!(create_args(&[]).session_start, None);

    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "loop",
        "update",
        "--project",
        "ProjectAlpha",
        "--loop",
        "daily-standup",
        "--name",
        "Renamed",
    ])
    .expect("loop update must parse");
    match parsed.command {
        Some(crate::cli::Commands::Loop(LoopArgs {
            command: LoopCommand::Update(args),
        })) => {
            assert_eq!(args.session_start, None);
            assert_eq!(args.session_start.map(LoopSessionStart::from), None);
        }
        _ => panic!("expected `loop update`"),
    }
}

/// AC-3 - an unrecognized value is a clap parse error naming both accepted
/// values. No hand-written validation.
#[test]
fn session_start_rejects_an_unknown_value_and_lists_the_accepted_ones() {
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "loop",
        "create",
        "--project",
        "ProjectAlpha",
        "--name",
        "Daily Standup",
        "--cron",
        "0 9 * * *",
        "--room",
        "wg-1-dev-team",
        "--prompt",
        "Summarize status",
        "--session-start",
        "resume",
    ]);
    let error = match parsed {
        Ok(_) => panic!("an unknown session-start value must not parse"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("fresh"), "error must list `fresh`:\n{error}");
    assert!(
        error.contains("accumulate"),
        "error must list `accumulate`:\n{error}"
    );
}

/// AC-4 (zero-effect) - adding the new flag does not disturb the busy flag
/// parsed from the same command line.
#[test]
fn session_start_does_not_disturb_the_busy_coordinator_flag() {
    let without = create_args(&["--busy-coordinator", "skip"]);
    let with = create_args(&[
        "--busy-coordinator",
        "skip",
        "--session-start",
        "accumulate",
    ]);

    assert_eq!(without.busy_coordinator, Some(BusyCoordinatorCli::Skip));
    assert_eq!(with.busy_coordinator, without.busy_coordinator);
    assert_eq!(with.force_inject_when_busy, without.force_inject_when_busy);
    assert_eq!(without.session_start, None);
    assert_eq!(with.session_start, Some(SessionStartCli::Accumulate));
}
