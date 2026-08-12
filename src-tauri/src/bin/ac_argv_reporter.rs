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
//! The echo loop additionally stops when it has observed the control line
//! `AC_1271_STOP` (with any line ending): in a PTY the child's stdin is the
//! ConPTY pipe, which never reaches EOF while the app holds the master, so the
//! regression sends this control line through the normal `PtyManager::write`
//! path to end the reporter deterministically.
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
    let mut pending: Vec<u8> = Vec::new();
    const STOP: &[u8] = b"AC_1271_STOP";
    loop {
        match std::io::stdin().read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = stdout.write_all(&buf[..n]);
                let _ = stdout.flush();
                pending.extend_from_slice(&buf[..n]);
                if pending.windows(STOP.len()).any(|w| w == STOP) {
                    break;
                }
                // Keep only the last STOP.len()-1 bytes so a marker split across
                // two reads still matches, while a trailing CR (ConPTY cooked
                // input turns LF into CR) cannot displace the marker bytes.
                if pending.len() >= STOP.len() {
                    pending.drain(..pending.len() - (STOP.len() - 1));
                }
            }
        }
    }
    let exit_code = (args.iter().map(|arg| arg.len()).sum::<usize>() % 256) as i32;
    std::process::exit(exit_code);
}
