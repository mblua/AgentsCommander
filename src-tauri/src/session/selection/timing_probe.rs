//! #2475 - test-only timing instrumentation (the parent also gates it on test).
#![cfg(test)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// One recorded timed operation. `submitted` is the wrapper's own stamp.
#[derive(Debug, Clone)]
pub(crate) struct SelectionTimingSample {
    pub(crate) invocation: Option<u64>,
    pub(crate) command: &'static str,
    pub(crate) submitted: Instant,
    pub(crate) queue_wait: Duration,
    pub(crate) work: Duration,
    pub(crate) ok: bool,
    pub(crate) body_thread: ThreadId,
}

pub(crate) type SelectionTimingSink = Arc<dyn Fn(&SelectionTimingSample, &str) + Send + Sync>;

static SELECTION_TIMING_SINKS: Mutex<Vec<(u64, &'static str, SelectionTimingSink)>> =
    Mutex::new(Vec::new());
static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_INVOCATION: AtomicU64 = AtomicU64::new(1);

/// Deliver to EVERY matching sink. The `Arc`s are cloned under the mutex
/// and invoked after it is released, so a blocking sink never holds it.
pub(super) fn deliver(command: &str, sample: &SelectionTimingSample, line: &str) {
    let sinks: Vec<SelectionTimingSink> = SELECTION_TIMING_SINKS
        .lock()
        .map(|sinks| {
            sinks
                .iter()
                .filter(|(_, name, _)| *name == command)
                .map(|(_, _, sink)| Arc::clone(sink))
                .collect()
        })
        .unwrap_or_default();
    for sink in sinks {
        sink(sample, line);
    }
}

/// Registers a recording sink for one command; only this registration is
/// removed on drop.
pub(crate) struct TimingSinkRegistration(u64);

impl TimingSinkRegistration {
    /// A sink that only sees the production log line.
    pub(crate) fn new(command: &'static str, sink: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self::new_sample(command, Arc::new(move |_sample, line| sink(line)))
    }

    pub(crate) fn new_sample(command: &'static str, sink: SelectionTimingSink) -> Self {
        let id = NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed);
        SELECTION_TIMING_SINKS
            .lock()
            .expect("sinks")
            .push((id, command, sink));
        Self(id)
    }
}

impl Drop for TimingSinkRegistration {
    fn drop(&mut self) {
        if let Ok(mut sinks) = SELECTION_TIMING_SINKS.lock() {
            sinks.retain(|(id, _, _)| *id != self.0);
        }
    }
}

/// How many sinks are registered for `command` right now.
pub(crate) fn registered_sinks(command: &str) -> usize {
    SELECTION_TIMING_SINKS
        .lock()
        .map(|sinks| sinks.iter().filter(|(_, name, _)| *name == command).count())
        .unwrap_or(0)
}

pub(crate) fn timing_field_ms(line: &str, name: &str) -> f64 {
    line.split(' ')
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("{name} missing in {line}"))
}

/// Serializes whole bench runs against each other.
pub(crate) fn bench_serialization_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Simultaneous occupancy owned by one run: only bodies that inherited
/// this `Arc` contribute, and there is no reset.
#[derive(Debug, Default)]
pub(crate) struct Occupancy {
    in_flight: AtomicUsize,
    max: AtomicUsize,
}

impl Occupancy {
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub(crate) fn max(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
}

/// Counts one holder until dropped (success, error or unwind).
pub(crate) struct OccupancyGuard(Arc<Occupancy>);

impl OccupancyGuard {
    pub(crate) fn enter(occupancy: &Arc<Occupancy>) -> Self {
        let now = occupancy.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        occupancy.max.fetch_max(now, Ordering::SeqCst);
        Self(Arc::clone(occupancy))
    }
}

impl Drop for OccupancyGuard {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub(crate) struct BenchContext {
    pub(crate) invocation: u64,
    pub(crate) occupancy: Arc<Occupancy>,
}

impl BenchContext {
    pub(crate) fn new(occupancy: &Arc<Occupancy>) -> Self {
        Self {
            invocation: NEXT_INVOCATION.fetch_add(1, Ordering::Relaxed),
            occupancy: Arc::clone(occupancy),
        }
    }
}
