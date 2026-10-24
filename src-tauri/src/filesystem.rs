//! Disk reads for user-owned documents.

use std::path::Path;

/// Read the capture catalog without changing the platform's file semantics.
pub(crate) fn read_capture_catalog(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests;
