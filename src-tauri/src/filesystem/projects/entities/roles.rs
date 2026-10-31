//! Persisted Matrix Role bytes and paths; excludes rendering and root resolution.
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const ROLE_FILE_NAME: &str = "Role.md";

pub(crate) fn role_path(matrix: &Path) -> PathBuf {
    matrix.join(ROLE_FILE_NAME)
}

pub(crate) fn role_present(matrix: &Path) -> bool {
    crate::filesystem::config_locations::replica_role_exists(&role_path(matrix))
}

pub(crate) fn read_role(matrix: &Path) -> io::Result<String> {
    std::fs::read_to_string(role_path(matrix))
}

pub(crate) fn read_role_if_present(matrix: &Path) -> Option<String> {
    if super::directories::agent_matrix_role_present(&role_path(matrix)) {
        read_role(matrix).ok()
    } else {
        None
    }
}

pub(crate) fn write_created_role(matrix: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = role_path(matrix);
    super::publication::write_created_role(&path, bytes)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;
    Ok(path)
}

pub(crate) fn role_is_file(matrix: &Path) -> bool {
    role_path(matrix).is_file()
}

pub(crate) fn inspect_role_path(
    matrix: &Path,
) -> Result<(), crate::filesystem::directories::ExperimentPathFailure> {
    crate::filesystem::directories::inspect_experiment_path_chain(&role_path(matrix))
}

pub(crate) fn is_copy_role_entry(name: &str) -> bool {
    name == ROLE_FILE_NAME
}

pub(crate) fn validate_copy_role_kind(is_regular_file: bool) -> Result<(), String> {
    if is_regular_file {
        Ok(())
    } else {
        Err("Unexpected generic asset type: Role.md".into())
    }
}

pub(crate) fn require_copy_role(seen: bool) -> Result<(), String> {
    if seen {
        Ok(())
    } else {
        Err("Missing required Role.md".into())
    }
}

#[cfg(test)]
mod tests;
