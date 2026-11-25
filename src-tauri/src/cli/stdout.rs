//! CLI stdout edge: one locked line write with the `BrokenPipe` exit policy.
//!
//! `cli_println!` delegates here. A `BrokenPipe` write error exits the process
//! with code 0 and nothing on stderr; any other write error writes one line
//! `failed printing to stdout: <error>` to stderr and exits with code 1.

#[cfg(test)]
mod tests;

/// Writes `args` plus one LF to stdout under a single stdout lock.
#[track_caller]
pub fn print_stdout(args: std::fmt::Arguments<'_>) {
    let stdout = std::io::stdout();
    let mut guard = stdout.lock();
    write_stdout(&mut guard, args);
}

#[track_caller]
#[cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unwrap_used, clippy::expect_used)
)]
fn write_stdout(writer: &mut impl std::io::Write, args: std::fmt::Arguments<'_>) {
    write_stdout_with_error(writer, args, |error| {
        use std::io::Write as _;
        let stderr = std::io::stderr();
        let mut guard = stderr.lock();
        writeln!(guard, "failed printing to stdout: {error}")
    });
}

/// Writes one line; on failure decides the exit. `report` writes the diagnostic
/// of a non-`BrokenPipe` error once; its own failure is ignored.
#[cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unwrap_used, clippy::expect_used)
)]
fn write_stdout_with_error(
    writer: &mut impl std::io::Write,
    args: std::fmt::Arguments<'_>,
    report: impl FnOnce(&std::io::Error) -> std::io::Result<()>,
) {
    let Err(error) = writeln!(writer, "{args}") else {
        return;
    };
    let code = if error.kind() == std::io::ErrorKind::BrokenPipe {
        0
    } else {
        // Nothing is left to report a failed diagnostic to.
        let _ignored = report(&error);
        1
    };
    #[expect(
        clippy::exit,
        reason = "a failed stdout write ends the CLI: 0 for BrokenPipe, 1 otherwise"
    )]
    std::process::exit(code);
}
