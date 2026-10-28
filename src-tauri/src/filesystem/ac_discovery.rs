//! Physical reads and probes for discovery; paths belong to callers.

use std::path::Path;

pub(crate) fn discovery_config_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_replica_discovery_config(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}
