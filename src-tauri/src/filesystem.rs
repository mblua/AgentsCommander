//! Disk reads for user-owned documents and loop event path resolution.

use std::path::{Path, PathBuf};

pub(crate) mod attachments;
pub(crate) mod directories;
pub(crate) mod documents;
pub(crate) mod processes;
pub(crate) mod projects;
pub(crate) mod transcripts;

/// Read the capture catalog without changing the platform's file semantics.
pub(crate) fn read_capture_catalog(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Preserve canonical loop event paths, falling back to the supplied spelling.
pub(crate) fn canonical_loop_event_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.into())
}

/// Read persisted session text with unchanged platform errors.
pub(crate) fn read_session_container_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Ensure the snapshot directory with unchanged platform errors.
pub(crate) fn ensure_session_container_directory(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Write the operation's unique stage using the original filesystem semantics.
pub(crate) fn write_session_container_stage(path: &Path, bytes: String) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Replace the snapshot using the existing native rename operation.
pub(crate) fn replace_session_container_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, dst)
}

/// Remove only the supplied operation stage.
pub(crate) fn remove_session_container_stage(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Read snapshot bytes for typed publication readback.
pub(crate) fn read_session_container_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

#[cfg(test)]
pub(crate) fn corrupt_session_container_for_test(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, b"{invalid fixture publication")
}
#[cfg(test)]
mod tests;
