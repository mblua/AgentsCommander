//! Disk access for the user-picked Coding Agents export/import file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Largest import file accepted, in bytes (4 MiB).
pub(crate) const MAX_TRANSFER_BYTES: u64 = 4_194_304;

/// Reads a user-picked import file as UTF-8 text, refusing files over `max_bytes`.
pub(crate) fn read_transfer_file(path: &Path, max_bytes: u64) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("Failed to read import file: {e}"))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read import file: {e}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(format!(
            "Import file is too large. Maximum size is {max_bytes} bytes."
        ));
    }
    String::from_utf8(bytes).map_err(|_| "Import file is not valid UTF-8".to_string())
}

/// Writes `content` to the user-picked export path, replacing an existing file.
pub(crate) fn write_transfer_file(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content.as_bytes())
        .map_err(|e| format!("Failed to write export file: {e}"))
}

#[cfg(test)]
mod tests;
