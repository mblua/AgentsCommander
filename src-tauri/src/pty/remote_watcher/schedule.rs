use std::time::{Duration, Instant};

use super::{
    CiState, FailureKind, BACKOFF_CAP, CI_RUNNING_INTERVAL_SECS, INTERVAL_CEILING_SECS,
    INTERVAL_FLOOR_SECS, SECONDARY_BACKOFF_BASE,
};

pub(crate) fn clamp_interval_secs(name: &str, raw: u64) -> u64 {
    let clamped = raw.clamp(INTERVAL_FLOOR_SECS, INTERVAL_CEILING_SECS);
    if clamped != raw {
        log::debug!("[RemoteSweeper] {name} {raw} clamped to {clamped}");
    }
    clamped
}

pub(crate) fn due(now: Instant, next_due: Option<Instant>) -> bool {
    match next_due {
        Some(due_at) => now >= due_at,
        None => true,
    }
}

pub(crate) fn ci_base_interval(confirmed: Option<CiState>, dial: Duration) -> Duration {
    match confirmed {
        Some(CiState::Running) => Duration::from_secs(CI_RUNNING_INTERVAL_SECS),
        _ => dial,
    }
}

/// Backoff is per key and per axis. Doubling is the default; exactly two named
/// conditions jump straight to the cap (rate limiting and not authenticated),
/// because both are not transient and hammering after being told no is how an
/// annoyance becomes an account block. The `max(base, ..)` is load-bearing: a
/// dial clamped to 3600 s is above the 900 s cap, and without it a failing key
/// would re-query FASTER than a healthy one.
pub(crate) fn failure_interval(
    kind: FailureKind,
    base: Duration,
    current: Option<Duration>,
) -> Duration {
    match kind {
        FailureKind::RateLimited | FailureKind::NotAuthenticated => BACKOFF_CAP.max(base),
        FailureKind::SecondaryRateLimited => current
            .unwrap_or(SECONDARY_BACKOFF_BASE / 2)
            .saturating_mul(2)
            .min(BACKOFF_CAP)
            .max(base),
        FailureKind::Timeout | FailureKind::Incomplete | FailureKind::Other => current
            .unwrap_or(base)
            .saturating_mul(2)
            .min(BACKOFF_CAP)
            .max(base),
    }
}

/// The applied wait is `base + base * jitter()`, uniform over `[base, 2*base)`.
/// The offset is TRUNCATED to whole nanoseconds, not rounded: `Duration::mul_f64`
/// rounds, so a fraction just under 1.0 returns `base` itself and the range
/// would be closed at both ends. The clamp guards a misbehaving seam.
pub(crate) fn jitter_offset(base: Duration, fraction: f64) -> Duration {
    Duration::from_nanos((base.as_nanos() as f64 * fraction.clamp(0.0, 1.0)) as u64)
}

pub(crate) fn axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Ci => "CI",
        Axis::Staleness => "staleness",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Axis {
    Ci,
    Staleness,
}
