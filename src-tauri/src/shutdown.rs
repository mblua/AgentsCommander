use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STARTUP_PENDING: u8 = 0;
const STARTUP_COMMITTED: u8 = 1;
const STARTUP_CANCELLED: u8 = 2;

#[cfg(test)]
thread_local! {
    static INJECTED_ACTOR_START_FAILURE: std::cell::RefCell<Option<&'static str>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_injected_actor_start_failure<T>(
    actor: &'static str,
    action: impl FnOnce() -> T,
) -> T {
    struct ResetInjectedActorStartFailure;

    impl Drop for ResetInjectedActorStartFailure {
        fn drop(&mut self) {
            INJECTED_ACTOR_START_FAILURE.with(|slot| {
                slot.replace(None);
            });
        }
    }

    INJECTED_ACTOR_START_FAILURE.with(|slot| {
        assert!(
            slot.replace(Some(actor)).is_none(),
            "only one actor start failure may be injected per test thread"
        );
    });
    let _reset = ResetInjectedActorStartFailure;
    action()
}

#[cfg(test)]
fn injected_actor_start_failure(actor: &'static str) -> Option<std::io::Error> {
    INJECTED_ACTOR_START_FAILURE.with(|slot| {
        (*slot.borrow() == Some(actor)).then(|| {
            std::io::Error::other(format!(
                "injected acknowledged actor start failure: {actor}"
            ))
        })
    })
}

struct StartupDurableGate {
    state: AtomicU8,
    active_writers: AtomicUsize,
    wait_lock: Mutex<()>,
    wait_cv: Condvar,
    notify: tokio::sync::Notify,
}

impl StartupDurableGate {
    fn new(initial_state: u8) -> Self {
        Self {
            state: AtomicU8::new(initial_state),
            active_writers: AtomicUsize::new(0),
            wait_lock: Mutex::new(()),
            wait_cv: Condvar::new(),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn commit(&self) -> bool {
        match self.state.compare_exchange(
            STARTUP_PENDING,
            STARTUP_COMMITTED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {
                self.notify.notify_waiters();
                true
            }
            Err(STARTUP_COMMITTED) => true,
            Err(_) => false,
        }
    }

    fn cancel(&self) {
        self.state.store(STARTUP_CANCELLED, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn try_acquire(self: &Arc<Self>) -> Option<DurableWritePermit> {
        if self.state.load(Ordering::SeqCst) != STARTUP_COMMITTED {
            return None;
        }
        self.active_writers.fetch_add(1, Ordering::SeqCst);
        if self.state.load(Ordering::SeqCst) == STARTUP_COMMITTED {
            Some(DurableWritePermit {
                gate: Arc::clone(self),
            })
        } else {
            self.release_writer();
            None
        }
    }

    fn release_writer(&self) {
        if self.active_writers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.wait_cv.notify_all();
        }
    }

    fn wait_for_writers(&self, budget: Duration) -> bool {
        let guard = self
            .wait_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = self
            .wait_cv
            .wait_timeout_while(guard, budget, |_| {
                self.active_writers.load(Ordering::SeqCst) != 0
            })
            .unwrap_or_else(|error| error.into_inner());
        !result.1.timed_out() && self.active_writers.load(Ordering::SeqCst) == 0
    }
}

/// Retains the startup durable-write read side until one mutation finishes.
///
/// Cancellation first closes the gate to new permits. Startup teardown can
/// then wait for the active count to reach zero before reporting failure.
pub struct DurableWritePermit {
    gate: Arc<StartupDurableGate>,
}

impl Drop for DurableWritePermit {
    fn drop(&mut self) {
        self.gate.release_writer();
    }
}

/// Unified shutdown signal for all background tasks.
///
/// - Async tasks (MailboxPoller, web server) use `token()` with `tokio::select!`
/// - Native threads with own tokio runtimes (GitWatcher, DiscoveryBranchWatcher) also use `token()`
/// - Pure native threads (IdleDetector) use `is_cancelled()` which checks an AtomicBool
///
/// A single `trigger()` call cancels both mechanisms simultaneously.
///
/// ## Tasks NOT covered by this signal (die with tokio runtime):
/// - Wake-and-sleep cleanup loops (phone/mailbox.rs) — async, up to 600s timeout
/// - Follow-up injection loops (phone/mailbox.rs) — async, up to 30s timeout
/// - Credential injection (commands/session.rs) — one-shot async, 2s sleep
///
/// These run on Tauri's tokio runtime and are force-cancelled on runtime drop.
#[derive(Clone)]
pub struct ShutdownSignal {
    token: CancellationToken,
    flag: Arc<AtomicBool>,
    startup_gate: Arc<StartupDurableGate>,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            flag: Arc::new(AtomicBool::new(false)),
            startup_gate: Arc::new(StartupDurableGate::new(STARTUP_COMMITTED)),
        }
    }

    /// Construct a signal whose background actors wait for startup commit.
    pub fn new_startup_gated() -> Self {
        Self {
            token: CancellationToken::new(),
            flag: Arc::new(AtomicBool::new(false)),
            startup_gate: Arc::new(StartupDurableGate::new(STARTUP_PENDING)),
        }
    }

    /// Publish startup commit and release startup-owned background actors.
    pub fn commit_startup(&self) -> bool {
        self.startup_gate.commit()
    }

    /// Wait asynchronously until startup commits or shutdown cancels the actor.
    pub async fn wait_for_startup_commit(&self) -> bool {
        loop {
            match self.startup_gate.state.load(Ordering::SeqCst) {
                STARTUP_COMMITTED => return true,
                STARTUP_CANCELLED => return false,
                _ => {}
            }
            let notified = self.startup_gate.notify.notified();
            match self.startup_gate.state.load(Ordering::SeqCst) {
                STARTUP_COMMITTED => return true,
                STARTUP_CANCELLED => return false,
                _ => {}
            }
            tokio::select! {
                _ = notified => {}
                _ = self.token.cancelled() => return false,
            }
        }
    }

    /// Wait on a native actor thread until startup commits or cancellation wins.
    pub fn wait_for_startup_commit_blocking(&self) -> bool {
        loop {
            match self.startup_gate.state.load(Ordering::SeqCst) {
                STARTUP_COMMITTED => return true,
                STARTUP_CANCELLED => return false,
                _ => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    /// Acquire the gate immediately before one durable mutation.
    pub fn try_durable_write(&self) -> Option<DurableWritePermit> {
        self.startup_gate.try_acquire()
    }

    /// Wait for every durable mutation that held a permit when cancellation won.
    pub fn wait_for_durable_writes(&self, budget: Duration) -> bool {
        self.startup_gate.wait_for_writers(budget)
    }

    /// Trigger shutdown, close the durable-write gate, and cancel async actors.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.startup_gate.cancel();
        self.token.cancel();
    }

    /// For async tasks: returns the CancellationToken to use in tokio::select!
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// For native threads: cheap non-blocking check.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

fn spawn_acknowledged_with<S, T>(
    actor: &'static str,
    spawn: S,
    task: T,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    S: FnOnce(Box<dyn FnOnce() + Send + 'static>) -> std::io::Result<std::thread::JoinHandle<()>>,
    T: FnOnce(std::sync::mpsc::SyncSender<std::io::Result<()>>) + Send + 'static,
{
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let handle = spawn(Box::new(move || task(ready_tx)))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            let _ = handle.join();
            Err(std::io::Error::other(format!(
                "{actor} exited before acknowledging startup"
            )))
        }
    }
}

/// Spawn a named native actor and return only after its thread acknowledges.
pub(crate) fn spawn_acknowledged_thread(
    actor: &'static str,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if let Some(error) = injected_actor_start_failure(actor) {
        return Err(error);
    }

    spawn_acknowledged_with(
        actor,
        move |task| {
            std::thread::Builder::new()
                .name(actor.to_string())
                .spawn(task)
        },
        move |ready| {
            let _ = ready.send(Ok(()));
            body();
        },
    )
}

/// Spawn a named native actor with its own Tokio runtime.
///
/// The caller receives an error for either OS-thread creation or Tokio runtime
/// construction, and receives the join owner only after both have succeeded.
pub(crate) fn spawn_acknowledged_tokio_thread<F>(
    actor: &'static str,
    future: F,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: Future<Output = ()> + Send + 'static,
{
    #[cfg(test)]
    if let Some(error) = injected_actor_start_failure(actor) {
        return Err(error);
    }

    spawn_acknowledged_with(
        actor,
        move |task| {
            std::thread::Builder::new()
                .name(actor.to_string())
                .spawn(task)
        },
        move |ready| match tokio::runtime::Runtime::new() {
            Ok(runtime) => {
                if ready.send(Ok(())).is_ok() {
                    runtime.block_on(future);
                }
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        },
    )
}

/// #632 - run `f` on a scratch thread and wait up to `budget`. Returns `Ok(result)`
/// if it finished in time, `Err(Timeout)` if the budget elapsed (the thread is
/// abandoned and the OS reclaims it at process exit), or `Err(Disconnected)` if the
/// closure PANICKED (the worker dropped the sender without sending).
///
/// Distinguishing timeout from panic lets the caller log a slow reaper differently
/// from a crashed one (LOW-3). Bounds shutdown cleanup so the UI thread cannot block
/// for minutes. Abandoning the work is safe ONLY for sessions that got a Job Object;
/// see the caller's job-less warning (MED-2).
pub fn run_time_boxed<T, F>(
    budget: std::time::Duration,
    f: F,
) -> Result<T, std::sync::mpsc::RecvTimeoutError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(budget)
}

#[cfg(test)]
mod tests {
    use super::{
        run_time_boxed, spawn_acknowledged_with, ShutdownSignal, STARTUP_COMMITTED, STARTUP_PENDING,
    };
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn ok_when_work_finishes_in_budget() {
        assert_eq!(run_time_boxed(Duration::from_secs(5), || 42), Ok(42));
    }

    #[test]
    fn timeout_when_work_exceeds_budget() {
        let r = run_time_boxed(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(500));
            42
        });
        assert_eq!(r, Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn disconnected_when_closure_panics() {
        let r: Result<i32, _> = run_time_boxed(Duration::from_secs(5), || panic!("boom"));
        assert_eq!(r, Err(RecvTimeoutError::Disconnected));
    }

    #[test]
    fn acknowledged_actor_propagates_thread_creation_failure() {
        let error = spawn_acknowledged_with(
            "injected thread failure",
            |_task| Err(io::Error::other("thread limit")),
            |_ready| panic!("task must not run when spawn fails"),
        )
        .expect_err("thread creation failure must propagate");
        assert_eq!(error.to_string(), "thread limit");
    }

    #[test]
    fn acknowledged_actor_propagates_initialization_failure() {
        let error = spawn_acknowledged_with(
            "injected runtime failure",
            |task| std::thread::Builder::new().spawn(task),
            |ready| {
                let _ = ready.send(Err(io::Error::other("runtime unavailable")));
            },
        )
        .expect_err("runtime construction failure must propagate");
        assert_eq!(error.to_string(), "runtime unavailable");
    }

    #[test]
    fn uncommitted_cancellation_closes_in_flight_durable_write_gate() {
        let signal = ShutdownSignal::new_startup_gated();
        assert_eq!(
            signal.startup_gate.state.load(Ordering::SeqCst),
            STARTUP_PENDING
        );
        assert!(signal.try_durable_write().is_none());

        assert!(signal.commit_startup());
        assert_eq!(
            signal.startup_gate.state.load(Ordering::SeqCst),
            STARTUP_COMMITTED
        );
        let permit = signal
            .try_durable_write()
            .expect("committed writer receives a permit");
        let signal_for_cancel = signal.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_thread = Arc::clone(&cancelled);
        let cancel = std::thread::spawn(move || {
            signal_for_cancel.trigger();
            let drained = signal_for_cancel.wait_for_durable_writes(Duration::from_secs(2));
            cancelled_for_thread.store(true, Ordering::SeqCst);
            drained
        });

        std::thread::sleep(Duration::from_millis(20));
        assert!(
            !cancelled.load(Ordering::SeqCst),
            "the in-flight fixture still owns its durable permit"
        );
        assert!(signal.try_durable_write().is_none());
        drop(permit);
        assert!(cancel.join().expect("join cancellation fixture"));
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn cancellation_after_producer_selection_prevents_uncommitted_file_write() {
        let signal = ShutdownSignal::new_startup_gated();
        let temp = tempfile::tempdir().expect("durable gate tempdir");
        let sentinel = temp.path().join("must-not-exist");
        let signal_for_writer = signal.clone();
        let sentinel_for_writer = sentinel.clone();
        let (selected_tx, selected_rx) = std::sync::mpsc::sync_channel(1);
        let (continue_tx, continue_rx) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn(move || {
            selected_tx
                .send(())
                .expect("signal producer passed its outer selection");
            continue_rx.recv().expect("release in-flight producer");
            if let Some(_permit) = signal_for_writer.try_durable_write() {
                std::fs::write(&sentinel_for_writer, b"forbidden").expect("write gated sentinel");
            }
        });

        selected_rx.recv().expect("wait for in-flight producer");
        signal.trigger();
        continue_tx.send(()).expect("release cancelled producer");
        writer.join().expect("join cancelled producer");
        assert!(
            !sentinel.exists(),
            "an uncommitted producer must recheck the gate before durable mutation"
        );
    }
}
