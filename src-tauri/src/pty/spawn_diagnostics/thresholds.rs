use std::time::Duration;

use super::{
    DEFAULT_CONCURRENCY_WINDOW_MS, DEFAULT_HEAD_BYTES, DEFAULT_PAINT_FLOOR_BYTES,
    DEFAULT_STALL_TIMEOUT_MS, DEFAULT_STOP_ATTRIBUTION_MS,
};

/// Everything the diagnostics need to be told, resolved once from the environment and
/// then carried by each record, so no code path (and no test) reads a process-global.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Deadline for the startup report. Zero disables it.
    pub stall_timeout: Duration,
    pub paint_floor: u64,
    pub head_bytes: usize,
    pub concurrency_window: Duration,
    pub stop_attribution: Duration,
    /// How long after a stop request ConPTY's teardown repaint can still arrive. Only
    /// the startup TIMING stamps are suppressed during it; bytes keep counting, and the
    /// stall verdict is byte-based, so a stop that fails to kill the child cannot fake a
    /// stall on a session that is painting normally.
    pub teardown_suppression: Duration,
    pub startup_poll: Duration,
    pub steady_poll: Duration,
}

impl Thresholds {
    pub const DEFAULT: Self = Self {
        stall_timeout: Duration::from_millis(DEFAULT_STALL_TIMEOUT_MS),
        paint_floor: DEFAULT_PAINT_FLOOR_BYTES,
        head_bytes: DEFAULT_HEAD_BYTES as usize,
        concurrency_window: Duration::from_millis(DEFAULT_CONCURRENCY_WINDOW_MS),
        stop_attribution: Duration::from_millis(DEFAULT_STOP_ATTRIBUTION_MS),
        teardown_suppression: Duration::from_secs(2),
        startup_poll: Duration::from_millis(250),
        steady_poll: Duration::from_secs(2),
    };
}
