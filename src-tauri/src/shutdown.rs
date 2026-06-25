use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
        }
    }

    /// Trigger shutdown — cancels the token and sets the atomic flag.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::SeqCst);
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
    use super::run_time_boxed;
    use std::sync::mpsc::RecvTimeoutError;
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
}
