//! Bounded final session persistence for the exit path (#4929).
use std::future::Future;
use std::time::Duration;

use super::ExitHost;

pub(super) const FINAL_PERSIST_BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FinalPersistOutcome {
    Persisted,
    TimedOut,
}

/// Runs `persist` on `host` and gives up after `budget`. Logs the outcome.
pub(super) fn run_final_persist<H, F>(host: &H, persist: F, budget: Duration) -> FinalPersistOutcome
where
    H: ExitHost,
    F: Future<Output = ()>,
{
    // Build the timer inside the block_on future: `timeout()` calls `Handle::current()`
    // (tokio 1.54.1 sleep.rs:250) and the GTK thread has no runtime, so it must run when polled.
    let outcome = match host.block_on(async move { tokio::time::timeout(budget, persist).await }) {
        Ok(()) => FinalPersistOutcome::Persisted,
        Err(_) => FinalPersistOutcome::TimedOut,
    };
    match outcome {
        FinalPersistOutcome::Persisted => {
            log::info!("[shutdown] Session state persisted, process exiting");
        }
        FinalPersistOutcome::TimedOut => log::error!(
            "[shutdown] final session persistence step=persist_current_state did not finish \
                within {}s; exiting without a confirmed sessions.json write",
            budget.as_secs()
        ),
    }
    outcome
}

#[cfg(test)]
mod tests;
