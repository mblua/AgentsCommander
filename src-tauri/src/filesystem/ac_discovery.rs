//! Physical reads and probes for discovery; paths belong to callers.

use std::path::{Path, PathBuf};

pub(crate) fn discovery_config_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn read_replica_discovery_config(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn discovery_directory_entries(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(path)
}

pub(crate) fn discovery_path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn discovery_role_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn canonical_discovery_repo(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn read_team_discovery_config(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn team_discovery_entries(
    path: &Path,
) -> std::io::Result<impl Iterator<Item = std::io::Result<(PathBuf, std::ffi::OsString)>>> {
    std::fs::read_dir(path)
        .map(|entries| entries.map(|entry| entry.map(|entry| (entry.path(), entry.file_name()))))
}
