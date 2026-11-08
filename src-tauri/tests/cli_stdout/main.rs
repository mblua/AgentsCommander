//! #4016: exact stdout bytes of the exported `cli_println!` macro.
//!
//! No `std::io::Write` import: the macro must not need one at the call site.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "AC_I4016_CLI_STDOUT_MACRO_CHILD";
const CHILD_TEST: &str = "macro_payload_child";
const BEGIN: &[u8] = b"I4016_CLI_STDOUT_BEGIN\n";
const END: &[u8] = b"I4016_CLI_STDOUT_END";
const CHILD_DEADLINE: Duration = Duration::from_secs(10);
const EXPECTED: &[u8] = "\nñandú ✓\nk=7\nb-a\ncaptured\nline\n\n".as_bytes();

/// Emits the payload through the real macro; runs only in the bounded child.
#[test]
fn macro_payload_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let captured = "captured";
    agentscommander_lib::cli_println!("I4016_CLI_STDOUT_BEGIN");
    let empty: () = agentscommander_lib::cli_println!();
    let unicode: () = agentscommander_lib::cli_println!("ñandú {}", "✓");
    agentscommander_lib::cli_println!("{label}={value}", label = "k", value = 7);
    agentscommander_lib::cli_println!("{1}-{0}", "a", "b");
    agentscommander_lib::cli_println!("{captured}");
    agentscommander_lib::cli_println!("line\n");
    agentscommander_lib::cli_println!("I4016_CLI_STDOUT_END");
    assert_eq!((empty, unicode), ((), ()));
}

fn find_once(haystack: &[u8], needle: &[u8]) -> usize {
    let hits: Vec<usize> = haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "sentinel {:?} must appear exactly once in {:?}",
        String::from_utf8_lossy(needle),
        String::from_utf8_lossy(haystack)
    );
    hits[0]
}

fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).expect("drain child pipe");
        bytes
    })
}

#[test]
fn cli_println_writes_exact_bytes() {
    assert!(
        std::env::var_os(CHILD_ENV).is_none(),
        "{CHILD_ENV} is reserved for the child and must not be inherited"
    );
    let exe = std::env::current_exe().expect("current test executable");
    let mut child = Command::new(exe)
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn macro child");
    let stdout = drain(child.stdout.take().expect("child stdout"));
    let stderr = drain(child.stderr.take().expect("child stderr"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if started.elapsed() >= CHILD_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("macro child exceeded {CHILD_DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().expect("join child stdout");
    let stderr = stderr.join().expect("join child stderr");
    assert!(
        status.success(),
        "macro child failed: {status:?}; stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    let begin = find_once(&stdout, BEGIN) + BEGIN.len();
    let end = find_once(&stdout, END);
    assert!(begin <= end, "sentinels out of order");
    assert_eq!(
        String::from_utf8_lossy(&stdout[begin..end]),
        String::from_utf8_lossy(EXPECTED)
    );
    assert_eq!(&stdout[begin..end], EXPECTED);
}
