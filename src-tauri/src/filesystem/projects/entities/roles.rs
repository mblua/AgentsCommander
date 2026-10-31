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

#[cfg(test)]
mod tests;
