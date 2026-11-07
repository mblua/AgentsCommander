//! Sole owner of the user-managed `.ac/heatmap.json` history configuration.
//!
//! The file is never created, repaired or rewritten here. A missing file means
//! disabled; an unreadable or invalid one means disabled with a bounded
//! diagnostic.
// Dormant until the history runtime (#3532 P02) reads the configuration.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::filesystem::context_history as history_fs;
use crate::path_identity;

/// Configuration file name inside the Project AC Root.
pub(crate) const CONFIG_FILE_NAME: &str = "heatmap.json";
/// The only supported configuration version.
pub(crate) const CONFIG_VERSION: u64 = 1;
/// Retention used when the file does not set one.
pub(crate) const DEFAULT_RETENTION_DAYS: u32 = 30;
/// Inclusive retention bounds, in days.
pub(crate) const MIN_RETENTION_DAYS: u32 = 1;
pub(crate) const MAX_RETENTION_DAYS: u32 = 365;
/// Largest accepted configuration file.
pub(crate) const MAX_CONFIG_BYTES: usize = 16 * 1024;
/// Largest diagnostic kept for an unusable configuration, in characters.
pub(crate) const MAX_DIAGNOSTIC_CHARS: usize = 200;

const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;
const OBSOLETE_INTERVAL_FIELD: &str = "intervalSeconds";

/// Effective history configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryConfig {
    pub(crate) enabled: bool,
    pub(crate) retention_days: u32,
}

impl HistoryConfig {
    /// The configuration in force when the file is missing or unusable.
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
    /// No file: disabled, nothing created.
    Missing,
    /// The file was read and accepted.
    Loaded,
    /// The file exists but is unusable: disabled with a bounded diagnostic.
    Invalid(String),
}

/// Result of loading the configuration; never fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedHistoryConfig {
    pub(crate) config: HistoryConfig,
    pub(crate) status: ConfigStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHistoryConfig {
    version: u64,
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_retention_days")]
    retention_days: u32,
}

fn default_retention_days() -> u32 {
    DEFAULT_RETENTION_DAYS
}

fn bounded(diagnostic: String) -> String {
    diagnostic.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
}

fn invalid(diagnostic: String) -> LoadedHistoryConfig {
    LoadedHistoryConfig {
        config: HistoryConfig::DISABLED,
        status: ConfigStatus::Invalid(bounded(diagnostic)),
    }
}

/// `<ac_root>/heatmap.json`.
pub(crate) fn config_path(ac_root: &Path) -> PathBuf {
    ac_root.join(CONFIG_FILE_NAME)
}

/// Parse configuration bytes strictly: duplicate keys, unknown fields, the
/// obsolete `intervalSeconds`, another version or an out-of-range retention
/// are errors.
pub(crate) fn parse_history_config(bytes: &[u8]) -> Result<HistoryConfig, String> {
    let value = path_identity::parse_json_no_duplicates(bytes)
        .map_err(|reason| format!("invalid_json: {reason}"))?;
    if value.get(OBSOLETE_INTERVAL_FIELD).is_some() {
        return Err("obsolete_interval_seconds".to_string());
    }
    let raw: RawHistoryConfig =
        serde_json::from_value(value).map_err(|error| format!("invalid_config: {error}"))?;
    if raw.version != CONFIG_VERSION {
        return Err(format!("unsupported_version: {}", raw.version));
    }
    if !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&raw.retention_days) {
        return Err(format!(
            "retention_days_out_of_range: {}",
            raw.retention_days
        ));
    }
    Ok(HistoryConfig {
        enabled: raw.enabled,
        retention_days: raw.retention_days,
    })
}

/// Load `<ac_root>/heatmap.json` without creating, repairing or writing it.
pub(crate) fn load_history_config(ac_root: &Path) -> LoadedHistoryConfig {
    let path = config_path(ac_root);
    match path_identity::read_bounded_regular(&path, MAX_CONFIG_BYTES) {
        Ok((bytes, _identity)) => match parse_history_config(&bytes) {
            Ok(config) => LoadedHistoryConfig {
                config,
                status: ConfigStatus::Loaded,
            },
            Err(reason) => invalid(reason),
        },
        // The bounded read reports a missing file like any unsafe one; the
        // read-only physical probe tells them apart without creating anything.
        Err(reason) => match history_fs::inspect_history_database(&path) {
            Ok(None) => LoadedHistoryConfig {
                config: HistoryConfig::DISABLED,
                status: ConfigStatus::Missing,
            },
            _ => invalid(format!("unreadable: {reason}")),
        },
    }
}

#[cfg(test)]
mod tests;
