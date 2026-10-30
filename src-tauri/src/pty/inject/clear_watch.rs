//! Opt-in observation of the known Codex fresh-session failure family.
use crate::pty::backend::PtyBackend;
use crate::pty::input_route::PtyInputObservation;
use crate::pty::manager::{PtyInputPermit, PtyManager};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WATCH_DURATION: Duration = Duration::from_secs(45);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const FAILURE_PREFIX: &str = "Failed to start a fresh session through the app server:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearWatchOutcome {
    KnownFailure,
    CompleteNoNewFailure,
    ExecutionError,
}

impl ClearWatchOutcome {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::KnownFailure => {
                "codex_clear_failed: provider reported fresh-session startup failure"
            }
            Self::ExecutionError => "codex_clear_execution_error: observation incomplete",
            Self::CompleteNoNewFailure => "",
        }
    }

    pub(crate) fn into_result(self) -> Result<(), String> {
        match self {
            Self::CompleteNoNewFailure => Ok(()),
            failure => Err(failure.reason().into()),
        }
    }
}

/// The capture is already plain rendered rows. Bound work by existing viewport limits.
fn normalize_failure_rows(rows: &[String]) -> Option<String> {
    if rows.len() > usize::from(terminal_snapshot_renderer::MAX_ROWS) {
        return None;
    }
    let bytes = rows
        .iter()
        .try_fold(0usize, |bytes, row| bytes.checked_add(row.len()))?;
    if bytes > terminal_snapshot_renderer::MAX_CELLS * 4
        || rows
            .iter()
            .any(|row| row.chars().any(|ch| ch.is_control() && !ch.is_whitespace()))
    {
        return None;
    }
    Some(
        rows.iter()
            .flat_map(|row| row.chars())
            .filter(|ch| !ch.is_whitespace())
            .collect(),
    )
}

pub(crate) fn capture_rows(
    observation: &PtyInputObservation,
    backend: &dyn PtyBackend,
) -> Option<Vec<String>> {
    if !observation.is_current() {
        return None;
    }
    let rows = backend.get_screen_rows(observation.session_id());
    if !observation.is_current() {
        return None;
    }
    match rows {
        crate::pty::context_scrape::ScreenRowsRead::Rows(rows) => Some(rows),
        _ => None,
    }
}

pub(crate) struct ClearWatch {
    backend: Arc<dyn PtyBackend>,
    observation: PtyInputObservation,
    samples: FailureSamples,
    interval: Duration,
    #[cfg(test)]
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

struct FailureSamples {
    eligible: Option<bool>,
    first_enter: Option<Instant>,
    outcome: Option<ClearWatchOutcome>,
    duration: Duration,
}

impl Default for FailureSamples {
    fn default() -> Self {
        Self {
            eligible: None,
            first_enter: None,
            outcome: None,
            duration: WATCH_DURATION,
        }
    }
}

impl ClearWatch {
    pub(crate) fn from_permit(
        permit: &PtyInputPermit,
        manager: &Arc<Mutex<PtyManager>>,
    ) -> Result<Self, String> {
        let observation = permit
            .observation()
            .map_err(|_| ClearWatchOutcome::ExecutionError.reason().to_string())?;
        let backend = manager
            .lock()
            .map_err(|_| ClearWatchOutcome::ExecutionError.reason().to_string())?
            .backend_for_kind(observation.backend_kind());
        let watch = Self {
            backend,
            observation,
            samples: FailureSamples::default(),
            interval: SAMPLE_INTERVAL,
            #[cfg(test)]
            barrier: None,
        };
        Ok(watch)
    }

    pub(crate) fn submitted(&mut self) {
        self.samples.first_enter = Some(Instant::now());
    }

    pub(crate) fn seam_sample(&mut self, seam: &str) -> Result<Option<Vec<String>>, String> {
        let sampled_at = Instant::now();
        let rows = capture_rows(&self.observation, self.backend.as_ref());
        if seam == "T0" {
            self.samples.baseline(rows.as_deref())?;
        } else if let Some(outcome) = self.samples.sample_at(sampled_at, rows.as_deref()) {
            outcome.into_result()?;
        }
        Ok(rows)
    }

    pub(crate) async fn finish(&mut self) -> ClearWatchOutcome {
        let Some(entered) = self.samples.first_enter else {
            return ClearWatchOutcome::ExecutionError;
        };
        #[cfg(test)]
        if let Some(barrier) = self.barrier.take() {
            barrier.wait().await;
        }
        let deadline = entered + self.samples.duration;
        loop {
            let sampled_at = Instant::now();
            let rows = capture_rows(&self.observation, self.backend.as_ref());
            if let Some(outcome) = self.samples.sample_at(sampled_at, rows.as_deref()) {
                return outcome;
            }
            let next = (sampled_at + self.interval).min(deadline);
            tokio::time::sleep_until(tokio::time::Instant::from_std(next)).await;
        }
    }

    #[cfg(test)]
    pub(crate) fn test_barrier(&mut self, barrier: Option<Arc<tokio::sync::Barrier>>) {
        self.barrier = barrier;
    }

    #[cfg(test)]
    pub(crate) fn backend(&self) -> &dyn PtyBackend {
        self.backend.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn test_timing(&mut self, duration: Duration, interval: Duration) {
        self.samples.duration = duration;
        self.interval = interval;
    }
}

impl FailureSamples {
    fn failure_present(rows: Option<&[String]>) -> Option<bool> {
        let normalized = normalize_failure_rows(rows?)?;
        let prefix: String = FAILURE_PREFIX
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        // Searching the structural prefix ignores an optional leading square bullet.
        Some(normalized.contains(&prefix))
    }

    fn baseline(&mut self, rows: Option<&[String]>) -> Result<(), String> {
        self.eligible = Self::failure_present(rows).map(|present| !present);
        if self.eligible.is_none() {
            self.outcome = Some(ClearWatchOutcome::ExecutionError);
            return ClearWatchOutcome::ExecutionError.into_result();
        }
        Ok(())
    }

    fn sample_at(&mut self, now: Instant, rows: Option<&[String]>) -> Option<ClearWatchOutcome> {
        if self.outcome.is_some() {
            return self.outcome;
        }
        let present = Self::failure_present(rows);
        let outcome = match (self.first_enter, self.eligible, present) {
            (Some(_), Some(true), Some(true)) => Some(ClearWatchOutcome::KnownFailure),
            (Some(entered), Some(_), Some(present)) => {
                if !present {
                    self.eligible = Some(true);
                }
                (now.saturating_duration_since(entered) >= self.duration)
                    .then_some(ClearWatchOutcome::CompleteNoNewFailure)
            }
            _ => Some(ClearWatchOutcome::ExecutionError),
        };
        self.outcome = outcome;
        outcome
    }
}

#[cfg(test)]
mod tests;
