//! Non-spinning, observable retry pacing for persist recapture loops.
use std::time::Duration;

/// Retries that only yield before the first sleep.
const SPIN_RETRIES: u32 = 64;
/// First sleep; doubles per sleep up to `MAX_BACKOFF`.
const BACKOFF: Duration = Duration::from_millis(25);
const MAX_BACKOFF: Duration = Duration::from_secs(1);
/// The warn fires on this sleep (25+50+100+200+400 ms already slept, ~0.8 s).
const WARN_AFTER_SLEEPS: u32 = 6;

pub(super) struct RecaptureBackoff {
    site: &'static str,
    attempts: u32,
}

impl RecaptureBackoff {
    pub(super) fn new(site: &'static str) -> Self {
        Self { site, attempts: 0 }
    }

    /// Retries 1..=SPIN_RETRIES yield. Later retries sleep 25 ms doubling to 1 s.
    /// Logs one warn per instance, on sleep number WARN_AFTER_SLEEPS.
    pub(super) async fn wait(&mut self, reason: &'static str) {
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts <= SPIN_RETRIES {
            tokio::task::yield_now().await;
            return;
        }
        let sleeps = self.attempts - SPIN_RETRIES;
        let delay = BACKOFF
            .saturating_mul(1 << (sleeps - 1).min(6))
            .min(MAX_BACKOFF);
        if sleeps == WARN_AFTER_SLEEPS {
            log::warn!(
                "[persist] recapture not converging site={} reason={} attempts={}; \
                    backing off up to {}ms",
                self.site,
                reason,
                self.attempts,
                MAX_BACKOFF.as_millis()
            );
        }
        tokio::time::sleep(delay).await;
    }
}

/// Parks forever after logging: publication state cannot be read.
pub(super) async fn park_unreadable<T>(site: &'static str) -> T {
    log::error!(
        "[persist] publication state unreadable (registry or slot lock poisoned); \
            persistence parked site={site}"
    );
    std::future::pending().await
}

#[cfg(test)]
mod tests;
