//! CLI stdout edge: one locked line write with the `BrokenPipe` exit policy.
//!
//! `cli_println!` delegates here. A `BrokenPipe` write error exits the process
//! with code 0; any other write error panics with
//! `failed printing to stdout: <error>`. See #230 and #4016.

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
fn write_stdout(writer: &mut impl std::io::Write, args: std::fmt::Arguments<'_>) {
    if let Err(e) = writeln!(writer, "{args}") {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            #[expect(clippy::exit, reason = "BrokenPipe exits successfully")]
            std::process::exit(0);
        } else {
            panic!("failed printing to stdout: {}", e);
        }
    }
}
