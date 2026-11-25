use super::*;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Child-only switch; its value names the synthetic error mode.
const CHILD_ENV: &str = "AC_I4016_STDOUT_ERROR_CHILD_7D91C2";
const CHILD_TEST: &str = "cli::stdout::tests::broken_pipe_child";
const MODE_MARKER: &str = "I4016_STDOUT_CHILD_MODE_";
const UNEXPECTED_RETURN: &str = "I4016_STDOUT_UNEXPECTED_RETURN";
const REPORT_ATTEMPT: &str = "I5206_STDOUT_REPORT_ATTEMPT";
const PANIC_HOOK: &str = "I5206_STDOUT_PANIC_HOOK";
const ERROR_DISPLAY: &str = "i4016 synthetic stdout failure";
const CHILD_DEADLINE: Duration = Duration::from_secs(10);
/// What libtest itself prints on the child's stdout before the test exits. The
/// child always runs on one thread, so the banner includes the test name.
const HARNESS_STDOUT: &str = "\nrunning 1 test\ntest cli::stdout::tests::broken_pipe_child ... ";

fn render(args: std::fmt::Arguments<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    write_stdout(&mut out, args);
    out
}

#[test]
fn empty_arguments_write_only_lf() {
    assert_eq!(render(format_args!("")), b"\n");
}

#[test]
fn unicode_and_captured_named_positional_arguments_keep_exact_bytes() {
    let captured = "ñandú";
    assert_eq!(render(format_args!("{captured}")), "ñandú\n".as_bytes());
    assert_eq!(
        render(format_args!("{label}={value}", label = "k", value = 7)),
        b"k=7\n"
    );
    assert_eq!(render(format_args!("{1}-{0}", "a", "b")), b"b-a\n");
    assert_eq!(render(format_args!("→ {} ✓", 42)), "→ 42 ✓\n".as_bytes());
}

#[test]
fn argument_ending_in_lf_gets_one_more_lf() {
    assert_eq!(render(format_args!("line\n")), b"line\n\n");
}

#[test]
fn successful_write_returns_without_reporting() {
    let mut out = Vec::new();
    let mut reported = false;
    write_stdout_with_error(&mut out, format_args!("ok"), |_| {
        reported = true;
        Ok(())
    });
    assert_eq!(out, b"ok\n");
    assert!(!reported, "the diagnostic writer must not run on success");
}

/// Writer that fails every write with the selected kind.
struct FailingWriter(io::ErrorKind);

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(self.0, ERROR_DISPLAY))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn mode_kind(mode: &str) -> io::ErrorKind {
    match mode {
        "broken_pipe" => io::ErrorKind::BrokenPipe,
        "permission_denied" => io::ErrorKind::PermissionDenied,
        "write_zero" => io::ErrorKind::WriteZero,
        "other" | "report_fails" => io::ErrorKind::Other,
        unknown => panic!("unknown child error mode {unknown:?}"),
    }
}

/// Runs only inside the bounded child spawned by [`run_child`].
#[test]
fn broken_pipe_child() {
    let Some(mode) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let mode = mode.into_string().expect("child mode is UTF-8");
    let kind = mode_kind(&mode);
    eprintln!("{MODE_MARKER}{mode}");
    if mode == "report_fails" {
        std::panic::set_hook(Box::new(|_| eprintln!("{PANIC_HOOK}")));
        let mut attempts = 0;
        write_stdout_with_error(&mut FailingWriter(kind), format_args!("payload"), |_| {
            attempts += 1;
            eprintln!("{REPORT_ATTEMPT}{attempts}");
            Err(io::Error::other("diagnostic writer failed"))
        });
    } else {
        write_stdout(&mut FailingWriter(kind), format_args!("payload"));
    }
    panic!("{UNEXPECTED_RETURN}");
}

struct ChildOutcome {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).expect("drain child pipe");
        bytes
    })
}

fn run_child(mode: &str) -> ChildOutcome {
    assert!(
        std::env::var_os(CHILD_ENV).is_none(),
        "{CHILD_ENV} is reserved for the child and must not be inherited"
    );
    let exe = std::env::current_exe().expect("current test executable");
    let mut child = Command::new(exe)
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, mode)
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_TEST_THREADS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdout error child");
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
            panic!("stdout error child {mode} exceeded {CHILD_DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().expect("join child stdout");
    let stderr = stderr.join().expect("join child stderr");
    ChildOutcome {
        code: status.code(),
        stdout: String::from_utf8(stdout).expect("child stdout is UTF-8"),
        stderr: String::from_utf8(stderr).expect("child stderr is UTF-8"),
    }
}

fn assert_single_mode_marker(outcome: &ChildOutcome, mode: &str) {
    let expected = format!("{MODE_MARKER}{mode}");
    let markers: Vec<&str> = outcome
        .stderr
        .lines()
        .filter(|line| line.contains(MODE_MARKER))
        .collect();
    assert_eq!(
        markers,
        [expected.as_str()],
        "child stderr: {}",
        outcome.stderr
    );
    assert!(
        !outcome.stderr.contains(UNEXPECTED_RETURN),
        "write_stdout returned for {mode}: {}",
        outcome.stderr
    );
}

/// Both channels in full, byte for byte: stdout holds only the harness banner
/// and stderr exactly the mode marker followed by `lines`, each ending in one LF.
fn assert_channels(outcome: &ChildOutcome, mode: &str, lines: &[&str]) {
    assert_eq!(
        outcome.stdout, HARNESS_STDOUT,
        "child stdout (stderr: {:?})",
        outcome.stderr
    );
    let mut want = format!("{MODE_MARKER}{mode}\n");
    for line in lines {
        want.push_str(line);
        want.push('\n');
    }
    assert_eq!(outcome.stderr, want, "child stderr");
}

#[test]
fn broken_pipe_exits_zero_without_panic() {
    let outcome = run_child("broken_pipe");
    assert_single_mode_marker(&outcome, "broken_pipe");
    assert_eq!(outcome.code, Some(0), "child stderr: {}", outcome.stderr);
    assert_channels(&outcome, "broken_pipe", &[]);
}

fn assert_error_mode_exits_one(mode: &str) {
    let outcome = run_child(mode);
    assert_single_mode_marker(&outcome, mode);
    assert_eq!(outcome.code, Some(1), "child stderr: {}", outcome.stderr);
    let diagnostic = format!("failed printing to stdout: {ERROR_DISPLAY}");
    assert_channels(&outcome, mode, &[diagnostic.as_str()]);
}

#[test]
fn permission_denied_exits_one_with_message() {
    assert_error_mode_exits_one("permission_denied");
}

#[test]
fn write_zero_exits_one_with_message() {
    assert_error_mode_exits_one("write_zero");
}

#[test]
fn other_error_exits_one_with_message() {
    assert_error_mode_exits_one("other");
}

#[test]
fn failed_diagnostic_is_tried_once_and_still_exits_one() {
    let outcome = run_child("report_fails");
    assert_single_mode_marker(&outcome, "report_fails");
    assert_eq!(outcome.code, Some(1), "child stderr: {}", outcome.stderr);
    let attempts: Vec<&str> = outcome
        .stderr
        .lines()
        .filter(|line| line.contains(REPORT_ATTEMPT))
        .collect();
    assert_eq!(
        attempts,
        [format!("{REPORT_ATTEMPT}1").as_str()],
        "child stderr: {}",
        outcome.stderr
    );
    assert!(
        !outcome.stderr.contains(PANIC_HOOK),
        "child stderr: {}",
        outcome.stderr
    );
    // The failing report wrote nothing itself: only its one attempt marker.
    let attempt = format!("{REPORT_ATTEMPT}1");
    assert_channels(&outcome, "report_fails", &[attempt.as_str()]);
}
