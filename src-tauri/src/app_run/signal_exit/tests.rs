//! Every case that raises a signal runs in a child process (this test binary,
//! re-run with `--exact`), so libtest and other tests never see the signals.
//! The parent only checks the child's exit status and stdout.

use super::*;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const CHILD_MARKER: &str = "AC5275_SIGNAL_CHILD";
const CHILD_WAIT: Duration = Duration::from_secs(10);

/// Full libtest name of `case` in this module.
fn case_name(case: &str) -> String {
    let path = module_path!();
    let path = path.split_once("::").map_or(path, |(_, rest)| rest);
    format!("{path}::{case}")
}

fn in_child(case: &str) -> bool {
    std::env::var(CHILD_MARKER).is_ok_and(|v| v == case_name(case))
}

/// Runs `case` alone in a child; returns its status, stdout and run time.
fn run_child(case: &str) -> (ExitStatus, String, Duration) {
    let name = case_name(case);
    let started = Instant::now();
    let mut child = Command::new(std::env::current_exe().expect("current exe"))
        .args([name.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_MARKER, &name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if started.elapsed() > CHILD_WAIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child {name} did not exit within {CHILD_WAIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let elapsed = started.elapsed();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("child stdout")
        .read_to_string(&mut stdout)
        .expect("read child stdout");
    (status, stdout, elapsed)
}

fn raise(signo: i32) {
    // SAFETY: `raise` takes no pointers and only sends `signo` to this
    // thread. It runs only inside a child process made for this one case.
    let rc = unsafe { libc::raise(signo) };
    assert_eq!(rc, 0, "raise({signo}) failed");
}

fn child_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime")
}

/// T1 child: install (readiness), raise, then drive `run` until `first` fires.
fn child_first(signo: i32) {
    let rt = child_runtime();
    let _enter = rt.enter();
    let streams = install().expect("install");
    raise(signo);
    let got = rt.block_on(async {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let first = move |s: &SignalExit| {
            let _ = tx.send(s.name);
        };
        let listener = run(streams, first, |_| {});
        tokio::select! {
            () = listener => None,
            got = rx => got.ok(),
            () = tokio::time::sleep(Duration::from_secs(5)) => None,
        }
    });
    // Leading newline: libtest leaves "test <name> ... " open on this line.
    println!("\nfirst {}", got.unwrap_or("none"));
}

fn parent_first(case: &str, name: &str) {
    let (status, stdout, _) = run_child(case);
    assert!(status.success(), "{case}: {status}; stdout: {stdout}");
    let line = format!("first {name}");
    assert!(
        stdout.lines().any(|l| l == line),
        "{case}: stdout: {stdout}"
    );
}

#[test]
fn t1_sigterm_reaches_first() {
    if in_child("t1_sigterm_reaches_first") {
        return child_first(libc::SIGTERM);
    }
    parent_first("t1_sigterm_reaches_first", "SIGTERM");
}

#[test]
fn t1_sighup_reaches_first() {
    if in_child("t1_sighup_reaches_first") {
        return child_first(libc::SIGHUP);
    }
    parent_first("t1_sighup_reaches_first", "SIGHUP");
}

#[test]
fn t1_sigint_reaches_first() {
    if in_child("t1_sigint_reaches_first") {
        return child_first(libc::SIGINT);
    }
    parent_first("t1_sigint_reaches_first", "SIGINT");
}

#[test]
fn t2_repeat_signals_reach_repeat() {
    const CASE: &str = "t2_repeat_signals_reach_repeat";
    if in_child(CASE) {
        let rt = child_runtime();
        let _enter = rt.enter();
        let streams = install().expect("install");
        raise(libc::SIGTERM);
        let firsts = Arc::new(AtomicUsize::new(0));
        let repeats = Arc::new(AtomicUsize::new(0));
        let (firsts_c, repeats_c) = (Arc::clone(&firsts), Arc::clone(&repeats));
        rt.block_on(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let first = move |_: &SignalExit| {
                firsts_c.fetch_add(1, Ordering::SeqCst);
                raise(libc::SIGTERM);
                raise(libc::SIGTERM);
            };
            let repeat = move |_: &SignalExit| {
                repeats_c.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(());
            };
            tokio::select! {
                () = run(streams, first, repeat) => {}
                _ = rx.recv() => {}
                () = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        });
        let (f, r) = (
            firsts.load(Ordering::SeqCst),
            repeats.load(Ordering::SeqCst),
        );
        println!("\ncounts first={f} repeats={r}");
        return;
    }
    let (status, stdout, _) = run_child(CASE);
    assert!(status.success(), "{status}; stdout: {stdout}");
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("counts "))
        .unwrap_or_else(|| panic!("no counts line; stdout: {stdout}"));
    let (first, repeats) = line.split_once(' ').expect("two counts");
    assert_eq!(first, "first=1", "stdout: {stdout}");
    let repeats: usize = repeats
        .strip_prefix("repeats=")
        .and_then(|n| n.parse().ok())
        .expect("repeat count");
    assert!(repeats >= 1, "repeats must be handled; stdout: {stdout}");
}

/// T3 negative control: a signal without a handler kills the child, so the
/// T1 harness would see a missing handler.
#[test]
fn t3_unhandled_signal_kills_child() {
    const CASE: &str = "t3_unhandled_signal_kills_child";
    if in_child(CASE) {
        let rt = child_runtime();
        let _enter = rt.enter();
        let _term = signal(SignalKind::terminate()).expect("sigterm listener");
        raise(libc::SIGHUP);
        println!("survived SIGHUP");
        return;
    }
    let (status, stdout, _) = run_child(CASE);
    assert_eq!(
        status.signal(),
        Some(libc::SIGHUP),
        "{status}; stdout: {stdout}"
    );
}

/// T4: the watchdog ends a blocked process even when its report step blocks.
#[test]
fn t4_watchdog_exits_blocked_process() {
    const CASE: &str = "t4_watchdog_exits_blocked_process";
    if in_child(CASE) {
        let lock = Arc::new(Mutex::new(()));
        let held = Arc::clone(&lock);
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _guard = held.lock().expect("hold lock");
            let _ = ready_tx.send(());
            loop {
                std::thread::park();
            }
        });
        ready_rx.recv().expect("lock held");
        spawn_watchdog(libc::SIGTERM, Duration::from_millis(100), || loop {
            std::thread::park();
        });
        let _blocked = lock.lock();
        println!("lock acquired unexpectedly");
        return;
    }
    let (status, stdout, elapsed) = run_child(CASE);
    assert_eq!(status.code(), Some(143), "{status}; stdout: {stdout}");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}
