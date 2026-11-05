//! Physical operations for the context history; the store owns paths, SQL and policy.
// Dormant until the history store (#3532 P01) consumes these operations.
#![allow(dead_code)]

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use crate::path_identity;

/// Maximum number of leading bytes read while inspecting a history database.
pub(crate) const HISTORY_HEADER_MAX_BYTES: usize = 100;

/// Physical facts about an existing history database file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryDatabaseInspection {
    /// Real file length in bytes.
    pub(crate) length: u64,
    /// Leading bytes, at most [`HISTORY_HEADER_MAX_BYTES`]; never padded.
    pub(crate) header: Vec<u8>,
}

fn unsafe_path(reason: String) -> io::Error {
    io::Error::other(format!("unsafe history path: {reason}"))
}

fn verify_directory(path: &Path) -> io::Result<()> {
    path_identity::verify_directory(path)
        .map(|_| ())
        .map_err(unsafe_path)
}

/// Create exactly `directory` (not its ancestors) unless a safe directory already exists.
pub(crate) fn ensure_history_directory(directory: &Path) -> io::Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(_) => return verify_directory(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = directory
        .parent()
        .ok_or_else(|| unsafe_path("missing_parent".to_string()))?;
    verify_directory(parent)?;
    match fs::create_dir(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    verify_directory(directory)
}

/// Inspect the database at `path` without creating anything.
///
/// Returns `None` when the file (or its directory) is missing. Opens read-only
/// and reads at most [`HISTORY_HEADER_MAX_BYTES`] bytes.
pub(crate) fn inspect_history_database(
    path: &Path,
) -> io::Result<Option<HistoryDatabaseInspection>> {
    let parent = path
        .parent()
        .ok_or_else(|| unsafe_path("missing_parent".to_string()))?;
    match fs::symlink_metadata(parent) {
        Ok(_) => verify_directory(parent)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let entry = match fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Reject links and non-regular entries (a FIFO would block `open`) before opening.
    if !entry.is_file() || path_identity::is_link_or_reparse(&entry) {
        return Err(unsafe_path("not_a_regular_file".to_string()));
    }
    let file = File::open(path)?;
    let identity =
        path_identity::verify_opened_regular_file(path, &file, false).map_err(unsafe_path)?;
    let mut header = Vec::with_capacity(HISTORY_HEADER_MAX_BYTES);
    file.take(HISTORY_HEADER_MAX_BYTES as u64)
        .read_to_end(&mut header)?;
    Ok(Some(HistoryDatabaseInspection {
        length: identity.metadata.len,
        header,
    }))
}

#[cfg(test)]
mod tests;
