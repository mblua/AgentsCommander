//! #2817 regression harness for the `tao` keyboard reentrancy deadlock.
//!
//! `tao 0.34.8` held a global, non-reentrant `KEY_EVENT_BUILDERS` mutex while
//! its `WM_KEYUP` branch called `PeekMessageW`. That peek delivers pending sent
//! messages, so a `WM_KILLFOCUS` arriving at that instant reentered the same
//! callback and the event-loop thread waited on itself forever.
//!
//! T1 pins the resolved `tao` floor and is portable. T2 drives the reentrancy
//! on a hidden window the test owns, and lives in `win_harness`.

use std::path::Path;

const TAO_FLOOR: (u64, u64, u64) = (0, 37, 1);

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The `version` of every `[[package]]` block named `tao`.
fn tao_versions(lock: &str) -> Vec<String> {
    lock.split("[[package]]")
        .filter(|block| block.lines().any(|line| line == "name = \"tao\""))
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("version = \""))
                .map(|rest| rest.trim_end_matches('"').to_string())
        })
        .collect()
}

#[test]
fn resolved_tao_is_at_least_0_37_1() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|err| panic!("#2817: cannot read {}: {err}", lock_path.display()));
    let versions = tao_versions(&lock);
    assert_eq!(
        versions.len(),
        1,
        "#2817: expected exactly one resolved tao package, found {versions:?}; \
         a second tao can hide the KEY_EVENT_BUILDERS reentrancy deadlock"
    );
    let found = &versions[0];
    let parsed = parse_version(found)
        .unwrap_or_else(|| panic!("#2817: unparseable tao version {found:?} in Cargo.lock"));
    assert!(
        parsed >= TAO_FLOOR,
        "#2817: resolved tao is {found}, below 0.37.1; tao < 0.37.1 reintroduces \
         the KEY_EVENT_BUILDERS reentrancy deadlock"
    );
}

#[cfg(windows)]
mod win_harness {
    use std::io::Write;
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use tao::event_loop::{EventLoop, EventLoopBuilder};
    use tao::platform::windows::{EventLoopBuilderExtWindows, WindowExtWindows};
    use tao::window::{Window, WindowBuilder};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetQueueStatus, PeekMessageW, SendMessageW, MSG, PM_NOREMOVE, QS_SENDMESSAGE, WM_KEYUP,
        WM_KILLFOCUS,
    };

    /// One deadline for setup, body and teardown.
    const WATCHDOG_DEADLINE: Duration = Duration::from_secs(25);
    /// Own bound of every inner wait; shorter than the watchdog so an inner
    /// expiry fails as a named assertion or cleanup error, never as an abort.
    const STEP_BOUND: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(5);

    // Stage reached, printed by the watchdog. 1 and 2 are written on entry to
    // the step, so a hang inside setup names the step that hung.
    const STAGE_EVENT_LOOP: usize = 1;
    const STAGE_WINDOW: usize = 2;
    const STAGE_PENDING_OBSERVED: usize = 3;
    const STAGE_OUTER_SEND_ISSUED: usize = 4;
    const STAGE_OUTER_SEND_RETURNED: usize = 5;
    const STAGE_DRAIN: usize = 6;
    const STAGE_SENDER_SETTLED: usize = 7;
    const STAGE_RESOURCES_DROPPED: usize = 8;

    const VK_SHIFT: usize = 0x10;
    /// The `lparam` of the incident's `WM_KEYUP`.
    const INCIDENT_KEYUP_LPARAM: isize = 0x80b6_0001;

    /// Everything that can hang on drop. Owned above `catch_unwind` so that
    /// unwinding drops none of it and `teardown` alone decides the order.
    #[derive(Default)]
    struct Owned {
        event_loop: Option<EventLoop<()>>,
        window: Option<Window>,
        sender: Option<JoinHandle<()>>,
    }

    /// tao hands out `isize`; `windows-sys 0.59` wants a raw pointer.
    fn raw_hwnd(hwnd: isize) -> HWND {
        hwnd as HWND
    }

    /// High word of `GetQueueStatus`: a sent message is in the queue right
    /// now. The low word only reports arrivals since the previous call.
    fn sent_message_pending() -> bool {
        let status = unsafe { GetQueueStatus(QS_SENDMESSAGE) };
        (status >> 16) & QS_SENDMESSAGE != 0
    }

    fn wait_until(bound: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + bound;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
    }

    /// Writes the stderr handle directly: a spawned thread inherits libtest's
    /// capture, so `eprintln!` would be buffered and lost at `abort()`.
    fn write_stderr(line: &str) {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "{line}");
        let _ = stderr.flush();
    }

    fn arm_watchdog(done: Arc<AtomicBool>, stage: Arc<AtomicUsize>) -> JoinHandle<()> {
        thread::spawn(move || {
            if wait_until(WATCHDOG_DEADLINE, || done.load(Ordering::SeqCst)) {
                return;
            }
            write_stderr(&format!(
                "#2817 watchdog abort: the WM_KEYUP send with a pending WM_KILLFOCUS did not \
                 finish; stage reached {}, deadline {} s. Stage 4 is the tao keyboard \
                 reentrancy deadlock, 1-3 an environment failure, 6-8 a teardown hang.",
                stage.load(Ordering::SeqCst),
                WATCHDOG_DEADLINE.as_secs()
            ));
            std::process::abort();
        })
    }

    fn spawn_killfocus_sender(hwnd: isize, kf_returned: Arc<AtomicBool>) -> JoinHandle<()> {
        thread::spawn(move || {
            // Blocks until the window thread processes it, so the message
            // stays pending-sent against that thread.
            unsafe { SendMessageW(raw_hwnd(hwnd), WM_KILLFOCUS, 0, 0) };
            kf_returned.store(true, Ordering::SeqCst);
        })
    }

    fn body(owned: &mut Owned, stage: &AtomicUsize, kf_returned: &Arc<AtomicBool>) {
        stage.store(STAGE_EVENT_LOOP, Ordering::SeqCst);
        let event_loop = owned
            .event_loop
            .insert(EventLoopBuilder::<()>::new().with_any_thread(true).build());
        stage.store(STAGE_WINDOW, Ordering::SeqCst);
        let window = WindowBuilder::new()
            .with_visible(false)
            .build(event_loop)
            .expect("#2817: cannot create the hidden harness window");
        let hwnd = owned.window.insert(window).hwnd();

        owned.sender = Some(spawn_killfocus_sender(hwnd, Arc::clone(kf_returned)));
        assert!(
            wait_until(STEP_BOUND, sent_message_pending),
            "PENDING_SENT_NEVER_OBSERVED: no sent message became pending within {} s",
            STEP_BOUND.as_secs()
        );
        stage.store(STAGE_PENDING_OBSERVED, Ordering::SeqCst);
        assert!(
            sent_message_pending(),
            "PENDING_SENT_LOST_BEFORE_OUTER_SEND: the pending WM_KILLFOCUS was consumed early"
        );

        stage.store(STAGE_OUTER_SEND_ISSUED, Ordering::SeqCst);
        // A same-thread send calls the window procedure directly, without
        // pumping the queue.
        unsafe { SendMessageW(raw_hwnd(hwnd), WM_KEYUP, VK_SHIFT, INCIDENT_KEYUP_LPARAM) };
        stage.store(STAGE_OUTER_SEND_RETURNED, Ordering::SeqCst);
        assert!(
            !sent_message_pending(),
            "PENDING_SENT_STILL_SET_AFTER_OUTER_SEND: the reentrancy path is no longer \
             exercised; tao's WM_KEYUP branch did not dispatch the pending WM_KILLFOCUS"
        );
    }

    /// `PeekMessageW` dispatches pending sent messages whatever its filter;
    /// this is the only thing in teardown that releases the sender.
    fn drain_sent_messages(hwnd: isize) -> bool {
        wait_until(STEP_BOUND, || {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            unsafe { PeekMessageW(&mut msg, raw_hwnd(hwnd), 0, 0, PM_NOREMOVE) };
            !sent_message_pending()
        })
    }

    fn settle_sender(sender: JoinHandle<()>, kf_returned: &AtomicBool) -> Option<&'static str> {
        if !wait_until(STEP_BOUND, || kf_returned.load(Ordering::SeqCst)) {
            // Still blocked in its send: detach, never join. It dies with
            // this test binary.
            drop(sender);
            return Some("SENDER_DETACHED");
        }
        // The still-armed watchdog bounds this join.
        sender.join().err().map(|_| "SENDER_JOIN_FAILED")
    }

    /// Same on the success path and the caught-panic path. Every step runs
    /// even after an earlier one fails; the watchdog is disarmed last.
    fn teardown(
        owned: &mut Owned,
        stage: &AtomicUsize,
        kf_returned: &AtomicBool,
        done: &AtomicBool,
        watchdog: JoinHandle<()>,
    ) -> Vec<String> {
        let mut failures = Vec::new();
        let mut fail = |name: &str, at: usize| failures.push(format!("{name} (stage {at})"));

        stage.store(STAGE_DRAIN, Ordering::SeqCst);
        let hwnd = owned.window.as_ref().map_or(0, |window| window.hwnd());
        if !drain_sent_messages(hwnd) {
            fail("DRAIN_DEADLINE_EXPIRED", STAGE_DRAIN);
        }

        stage.store(STAGE_SENDER_SETTLED, Ordering::SeqCst);
        let settled = owned
            .sender
            .take()
            .and_then(|sender| settle_sender(sender, kf_returned));
        if let Some(name) = settled {
            fail(name, STAGE_SENDER_SETTLED);
        }

        stage.store(STAGE_RESOURCES_DROPPED, Ordering::SeqCst);
        drop(owned.window.take());
        drop(owned.event_loop.take());

        done.store(true, Ordering::SeqCst);
        if watchdog.join().is_err() {
            fail("WATCHDOG_JOIN_FAILED", STAGE_RESOURCES_DROPPED);
        }
        failures
    }

    #[test]
    fn wm_keyup_peek_does_not_deadlock_on_reentrant_wm_killfocus() {
        let done = Arc::new(AtomicBool::new(false));
        let stage = Arc::new(AtomicUsize::new(0));
        let kf_returned = Arc::new(AtomicBool::new(false));
        // Armed before any tao call: setup can hang too.
        let watchdog = arm_watchdog(Arc::clone(&done), Arc::clone(&stage));
        let mut owned = Owned::default();

        let outcome = catch_unwind(AssertUnwindSafe(|| body(&mut owned, &stage, &kf_returned)));
        let failures = teardown(&mut owned, &stage, &kf_returned, &done, watchdog);

        for failure in &failures {
            write_stderr(&format!("#2817 cleanup failure: {failure}"));
        }
        if let Err(original) = outcome {
            resume_unwind(original);
        }
        assert!(
            failures.is_empty(),
            "CLEANUP_FAILED: {}",
            failures.join(", ")
        );
    }
}
