//! #1271 - repository-owned native-child argv reporter for the Windows
//! PowerShell managed-native regression (`tests/pty_powershell_managed_native.rs`).
//!
//! A plain std-only console app with three jobs:
//!
//! 1. Print each argv element (after the program) on its own length-prefixed
//!    line: `<byte-len>:<raw value>` followed by `\n`. The length prefix makes
//!    the output lossless for any value, including one containing colons.
//! 2. Echo stdin to stdout until EOF, so the PTY-input half of the managed
//!    native-child regression has a real observer.
//! 3. Exit with a deterministic code derived from its own arguments: the sum of
//!    the UTF-8 byte lengths of all arguments, modulo 256. The test computes
//!    the same value, so the exit-code assertion is exact.
//!
//! Cargo auto-discovers `src/bin/*.rs`, so no `Cargo.toml` entry is needed. The
//! integration test locates the built binary at compile time through the
//! Cargo-provided `CARGO_BIN_EXE_ac_argv_reporter` environment variable.

use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout();
    for arg in &args {
        let line = format!("{}:{}\n", arg.len(), arg);
        let _ = stdout.write_all(line.as_bytes());
        let _ = stdout.flush();
    }
    let mut buf = [0u8; 4096];
    loop {
        match std::io::stdin().read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = stdout.write_all(&buf[..n]);
                let _ = stdout.flush();
            }
        }
    }
    let exit_code = (args.iter().map(|arg| arg.len()).sum::<usize>() % 256) as i32;
    std::process::exit(exit_code);
}
