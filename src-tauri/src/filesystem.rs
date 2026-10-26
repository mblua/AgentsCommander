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

#[cfg(test)]
mod tests;
