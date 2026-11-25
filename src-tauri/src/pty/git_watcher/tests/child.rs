//! #5206 P03: the one child-process relaunch of the observer tests (own process logger).
//!
//! The parent owns the child, its private files and a process guard from spawn to a
//! confirmed reap. Normal wait: origin + 45 s. Cleanup: one kill, then `try_wait` for at
//! most 10 s, never past origin + 55 s. An unconfirmed reap marks the guard fatal before it
//! is released, and every later call refuses to spawn.

use std::io::Write as _;
use std::process::{Child, ExitStatus};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const P03_CHILD: &str = "AC5206_P03_GIT_CHILD";
const P03_ARGS: [&str; 4] = ["--exact", "--show-output", "--test-threads", "1"];
const P03_DONE: &str = "AC5206_P03_DONE";
const WAIT: Duration = Duration::from_secs(45);
const CLEANUP: Duration = Duration::from_secs(10);
const LIMIT: Duration = Duration::from_secs(55);
const STEP: Duration = Duration::from_millis(20);

pub(crate) static P03_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Held from spawn to confirmed reap; `true` once a child's reap was not confirmed.
static P03_FATAL: Mutex<bool> = Mutex::new(false);

struct P03Log;

impl log::Log for P03Log {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() == log::Level::Error
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            P03_ERRORS.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

/// Sleeps one poll step, never past `until`.
fn nap(until: Instant) {
    std::thread::sleep(STEP.min(until.saturating_duration_since(Instant::now())));
}

/// One kill, then `try_wait` until `until`: (kill result, status reaped in time).
/// Never blocks; a reap seen after `until` does not count.
fn p03_reap<C>(
    child: &mut C,
    kill: fn(&mut C) -> std::io::Result<()>,
    try_wait: fn(&mut C) -> std::io::Result<Option<ExitStatus>>,
    until: Instant,
) -> (String, Option<ExitStatus>) {
    let killed = format!("{:?}", kill(child));
    loop {
        let seen = try_wait(child);
        if Instant::now() > until {
            return (killed, None);
        }
        if let Ok(Some(status)) = seen {
            return (killed, Some(status));
        }
        nap(until);
    }
}

/// Owns the child, its private files and the guard; cleans up once, also on Drop.
struct P03Child {
    child: Child,
    origin: Instant,
    dir: Option<tempfile::TempDir>,
    done: bool,
    fatal: MutexGuard<'static, bool>,
}

impl P03Child {
    /// Bounded kill and reap; keeps the files and marks the guard fatal if unconfirmed.
    fn cleanup(&mut self, why: &str) -> String {
        self.done = true;
        let start = Instant::now();
        let until = (start + CLEANUP).min(self.origin + LIMIT);
        let (killed, reaped) = p03_reap(&mut self.child, Child::kill, Child::try_wait, until);
        *self.fatal |= reaped.is_none();
        let files = self.dir.take().map(tempfile::TempDir::keep);
        format!(
            "{why}; pid {}, cleanup {:?}..{:?} after spawn, kill {killed}, reap {reaped:?}, \
             fatal {}, files {files:?}\n",
            self.child.id(),
            start - self.origin,
            self.origin.elapsed(),
            *self.fatal,
        )
    }
}

impl Drop for P03Child {
    fn drop(&mut self) {
        if !self.done {
            let report = self.cleanup("P03 child dropped unfinished");
            let _ = std::io::stderr().write_all(report.as_bytes());
        }
    }
}

/// Held by the child test; on a clean (non-panicking) end it writes the one receipt.
pub(crate) struct P03Done(String);

impl Drop for P03Done {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            // Through the test capture, so --show-output prints it after the result line.
            println!("{P03_DONE} {}", self.0);
        }
    }
}

/// The child's private stdout must show exactly this test passing, once, and its receipt.
fn p03_oracle(stdout: &str, fqn: &str) -> Result<(), String> {
    let count = |want: &str| stdout.lines().filter(|l| *l == want).count();
    let summaries: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("test result: "))
        .collect();
    let receipts = stdout.lines().filter(|l| l.starts_with(P03_DONE)).count();
    let ok = count(&format!("test {fqn} ... ok")) == 1
        && summaries.len() == 1
        && summaries[0].starts_with("test result: ok. 1 passed; 0 failed; ")
        && receipts == 1
        && count(&format!("{P03_DONE} {fqn}")) == 1;
    if ok {
        Ok(())
    } else {
        Err(format!(
            "child {fqn}: stdout does not prove one passing run with its receipt"
        ))
    }
}

/// Reruns `module::name` in a child of this test binary (its own process logger). In the
/// child it returns the receipt token to hold until the end of the test; in the parent None.
pub(crate) fn p03_in_child(module: &str, name: &str) -> Option<P03Done> {
    let fqn = format!("{}::{name}", module.split_once("::").expect("crate path").1);
    if let Some(mark) = std::env::var_os(P03_CHILD) {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let want: Vec<&str> = std::iter::once(fqn.as_str()).chain(P03_ARGS).collect();
        if mark != fqn.as_str() || argv != want {
            let line = format!("invalid P03 child mark {mark:?} or argv {argv:?} for {fqn}\n");
            let _ = std::io::stderr().write_all(line.as_bytes());
            std::process::abort();
        }
        log::set_logger(&P03Log).expect("child installs the error logger");
        log::set_max_level(log::LevelFilter::Error);
        return Some(P03Done(fqn));
    }
    let fatal = P03_FATAL.lock().expect("P03 relaunch guard poisoned");
    assert!(
        !*fatal,
        "an earlier P03 child was not reaped; not spawning {fqn}"
    );
    let exe = std::env::current_exe().unwrap();
    assert!(exe.is_absolute(), "{exe:?}");
    let dir = tempfile::tempdir().unwrap();
    let out = |n: &str| std::fs::File::create(dir.path().join(n)).unwrap();
    std::fs::create_dir(dir.path().join("config")).unwrap();
    let origin = Instant::now();
    let child = std::process::Command::new(&exe)
        .arg(&fqn)
        .args(P03_ARGS)
        .env(P03_CHILD, &fqn)
        .env_remove("RUST_TEST_NOCAPTURE")
        .env("AGENTSCOMMANDER_CONFIG_DIR", dir.path().join("config"))
        .stdin(std::process::Stdio::null())
        .stdout(out("stdout"))
        .stderr(out("stderr"))
        .spawn()
        .expect("spawn the child test");
    let (dir, done) = (Some(dir), false);
    let mut owner = P03Child {
        child,
        origin,
        dir,
        done,
        fatal,
    };
    let status = loop {
        let seen = owner.child.try_wait();
        match seen {
            Ok(Some(status)) if origin.elapsed() <= WAIT => break status,
            Ok(None) if origin.elapsed() < WAIT => nap(origin + WAIT),
            other => {
                let report = owner.cleanup(&format!("child {fqn} unfinished in time: {other:?}"));
                drop(owner);
                panic!("{report}");
            }
        }
    };
    owner.done = true;
    let stdout = owner
        .dir
        .as_ref()
        .map(|d| std::fs::read_to_string(d.path().join("stdout")));
    let verdict = match stdout {
        _ if !status.success() => Err(format!("child {fqn} failed: {status}")),
        Some(Ok(text)) => p03_oracle(&text, &fqn),
        other => Err(format!("child {fqn}: stdout unreadable: {other:?}")),
    };
    if let Err(why) = verdict {
        let files = owner.dir.take().map(tempfile::TempDir::keep);
        drop(owner);
        panic!("{why}; files kept in {files:?}");
    }
    None
}
