//! Replica discovery reads, including permissive legacy config compatibility.

use std::path::Path;

pub(crate) fn discovery_replica_config_exists(dir: &Path) -> bool {
    crate::filesystem::ac_discovery::discovery_config_exists(&crate::config::config_file_path(dir))
}

pub(crate) fn read_discovery_replica_json(dir: &Path) -> Option<serde_json::Value> {
    crate::filesystem::ac_discovery::read_replica_discovery_config(
        &crate::config::config_file_path(dir),
    )
    .ok()
    .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
}

pub(crate) fn read_discovery_replica_context(dir: &Path) -> Result<Vec<String>, String> {
    let config_path = crate::config::config_file_path(dir);
    if !crate::filesystem::ac_discovery::discovery_config_exists(&config_path) {
        return Ok(vec![]);
    }

    let content = crate::filesystem::ac_discovery::read_replica_discovery_config(&config_path)
        .map_err(|e| format!("Failed to read config.json: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config.json: {e}"))?;

    let files = parsed
        .get("context")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(files)
}
