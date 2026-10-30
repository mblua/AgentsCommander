//! Persistent throttle cache for npm update checks.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const CACHE_FILE_NAME: &str = "update-check.json";

/// The last registry check and its published version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cache {
    pub(crate) last_checked_at: DateTime<Utc>,
    pub(crate) latest_version: String,
}

pub(crate) fn read(directory: Option<&Path>) -> Option<Cache> {
    let path = directory?.join(CACHE_FILE_NAME);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write(directory: Option<&Path>, cache: &Cache) {
    let Some(directory) = directory else { return };
    let path = directory.join(CACHE_FILE_NAME);
    let Ok(json) = serde_json::to_string_pretty(cache) else {
        return;
    };
    // Preserve the fixed sibling stage and best-effort native replacement.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests;
