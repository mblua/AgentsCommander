//! Physical watchdog for one bounded selection close: producer stamps, inclusive
//! S/A/R limits, wake flag and the gates opened on the first failure.

use super::deadline::ms;
use super::*;
use futures::task::ArcWake;
use std::sync::Condvar;

const ACK_LIMIT: Duration = Duration::from_secs(1);
const RETURN_LIMIT: Duration = Duration::from_secs(2);
const POST_LIMIT: Duration = Duration::from_secs(5);
const RESCUE_LIMIT: Duration = Duration::from_secs(5);

/// Physical stamps the evaluator reads; `joins` are the handles owned after R.
#[derive(Clone, Debug, Default)]
pub(super) struct Stamps {
    pub(super) s: Option<Instant>,
    pub(super) a: Option<Instant>,
    pub(super) r: Option<Instant>,
    pub(super) joins: Vec<(String, Option<Instant>)>,
}

/// First physical failure of the stamps at `now`, with inclusive limits.
pub(super) fn physical_failure(stamps: &Stamps, now: Instant) -> Option<String> {
    let s = stamps.s?;
    let ack = s + ACK_LIMIT;
    let a = match stamps.a {
        Some(a) if a > ack => return Some(format!("ack late by {:?}", a - ack)),
        Some(a) => a,
        None if now > ack => return Some("ack missing at S+1s".to_string()),
        None => return None,
    };
    let ret = a + RETURN_LIMIT;
    let r = match stamps.r {
        Some(r) if r > ret => return Some(format!("return late by {:?}", r - ret)),
        Some(r) => r,
        None if now > ret => return Some("return missing at A+2s".to_string()),
        None => return None,
    };
    let post = r + POST_LIMIT;
    stamps.joins.iter().find_map(|(id, joined)| match joined {
        Some(j) if *j > post => Some(format!("post/{id} joined late by {:?}", *j - post)),
        None if now > post => Some(format!("post/{id} still pending at R+5s")),
        _ => None,
    })
}

#[derive(Default)]
pub(super) struct Gate {
    pub(super) open: Mutex<bool>,
    pub(super) changed: Condvar,
}

impl Gate {
    pub(super) fn open(&self) {
        *self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    pub(super) fn wait(&self) {
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*open {
            open = self
                .changed
                .wait(open)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

#[derive(Default)]
pub(super) struct WatchState {
    pub(super) stamps: Stamps,
    pub(super) first_failure: Option<String>,
    pub(super) findings: Vec<String>,
    pub(super) rescue_at: Option<Instant>,
    pub(super) done: bool,
}

/// Wakes the driver from a waker, the watchdog or a native thread; permits are kept.
#[derive(Default)]
pub(super) struct WakeFlag {
    pub(super) woke: AtomicBool,
    pub(super) notify: tokio::sync::Notify,
}

impl ArcWake for WakeFlag {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.woke.store(true, Ordering::SeqCst);
        arc_self.notify.notify_one();
    }
}

pub(super) struct Watchdog {
    pub(super) state: Mutex<WatchState>,
    pub(super) changed: Condvar,
    pub(super) gates: Vec<Arc<Gate>>,
    pub(super) wake: Arc<WakeFlag>,
    pub(super) case: String,
}

impl Watchdog {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn stamp(&self, update: impl FnOnce(&mut Stamps)) {
        update(&mut self.lock().stamps);
        self.changed.notify_all();
    }

    pub(super) fn join_stamp(&self, id: &str) {
        let now = Instant::now();
        self.stamp(|stamps| {
            if let Some(slot) = stamps.joins.iter_mut().find(|(owned, _)| owned == id) {
                slot.1.get_or_insert(now);
            }
        });
    }

    pub(super) fn first_failure(&self) -> Option<String> {
        self.lock().first_failure.clone()
    }

    pub(super) fn fail(&self, state: &mut WatchState, finding: String, at: Instant) {
        let line = format!(
            "case={} watchdog {finding} stamps={:?}",
            self.case, state.stamps
        );
        eprintln!("{line}");
        if state.first_failure.is_none() {
            state.first_failure = Some(line);
            state.rescue_at = Some(at + RESCUE_LIMIT);
            for gate in &self.gates {
                gate.open();
            }
        } else {
            state.findings.push(line);
        }
        WakeFlag::wake_by_ref(&self.wake);
    }

    /// Runs on a blocking thread, which also keeps Tokio from auto-advancing time.
    pub(super) fn watch(&self, started: oneshot::Sender<()>) {
        let _ = started.send(());
        let mut state = self.lock();
        let mut reported = Vec::new();
        while !state.done {
            let now = Instant::now();
            if let Some(finding) = physical_failure(&state.stamps, now) {
                if !reported.contains(&finding) {
                    reported.push(finding.clone());
                    self.fail(&mut state, finding, now);
                }
            }
            if state.rescue_at.is_some_and(|at| now > at) {
                eprintln!(
                    "case={} rescue expired; first={:?} stamps={:?}",
                    self.case, state.first_failure, state.stamps
                );
                state.rescue_at = None;
            }
            state = self
                .changed
                .wait_timeout(state, ms(5))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    pub(super) fn finish(&self) {
        let mut state = self.lock();
        if let Some(finding) = physical_failure(&state.stamps, Instant::now()) {
            if state.first_failure.is_none() {
                self.fail(&mut state, finding, Instant::now());
            }
        }
        state.done = true;
        self.changed.notify_all();
    }
}

/// Stops the watchdog thread and opens its gates if the test panics before `finish`,
/// so the runtime can shut down and the first assertion stays the reported failure.
pub(super) struct StopOnPanic(pub(super) Arc<Watchdog>);

impl Drop for StopOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for gate in &self.0.gates {
                gate.open();
            }
            self.0.lock().done = true;
            self.0.changed.notify_all();
        }
    }
}
