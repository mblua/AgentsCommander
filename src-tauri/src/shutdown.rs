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
/// - Wake-and-sleep cleanup loops (phone/mailbox.rs) - async, up to 600s timeout
/// - Follow-up injection loops (phone/mailbox.rs) - async, up to 30s timeout
/// - Credential injection (commands/session.rs) - one-shot async, 2s sleep
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

    /// Trigger shutdown - cancels the token and sets the atomic flag.
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
