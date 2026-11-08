//! Runtime values of the context-history configuration.
//!
//! The configuration comes from the app-wide Settings (#4049); this module
//! reads no file and holds no cache. Legacy project files are ignored.

/// Retention in force when history is disabled.
const DEFAULT_RETENTION_DAYS: u32 = 30;

const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// Effective history configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryConfig {
    pub(crate) enabled: bool,
    pub(crate) retention_days: u32,
}

impl HistoryConfig {
    /// The configuration in force when Settings are unavailable.
    pub(crate) const DISABLED: Self = Self {
        enabled: false,
        retention_days: DEFAULT_RETENTION_DAYS,
    };

    /// Retention window in milliseconds.
    pub(crate) fn retention_ms(&self) -> i64 {
        i64::from(self.retention_days) * MS_PER_DAY
    }
}

/// Where the effective configuration came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigStatus {
    /// No configuration source: disabled.
    Missing,
    /// A Settings snapshot was read.
    Loaded,
    /// The source is unavailable: disabled with a bounded, path-free diagnostic.
    Invalid(String),
}

/// Result of loading the configuration; never fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedHistoryConfig {
    pub(crate) config: HistoryConfig,
    pub(crate) status: ConfigStatus,
}

#[cfg(test)]
mod tests;
