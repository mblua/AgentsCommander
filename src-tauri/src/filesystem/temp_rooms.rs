//! Disk access for temp-room folders. Knows no naming rule.

use std::path::Path;

/// One child of the temp-room base dir.
pub(crate) struct ChildEntry {
    pub(crate) name: String,
    /// A real directory: not a file, symlink or reparse point.
    pub(crate) real_dir: bool,
}

/// Every UTF-8 child name of `base` (files, links and dirs); a missing base is empty.
pub(crate) fn list_child_entries(base: &Path) -> Result<Vec<ChildEntry>, String> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read temp room base: {e}")),
    };
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read temp room base: {e}"))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let real_dir = std::fs::symlink_metadata(entry.path()).is_ok_and(|metadata| {
            metadata.is_dir() && !crate::path_identity::is_link_or_reparse(&metadata)
        });
        children.push(ChildEntry { name, real_dir });
    }
    Ok(children)
}

/// Create `path` atomically; `AlreadyExists` when it is already taken.
pub(crate) fn create_dir_exclusive(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(path)
}

#[cfg(test)]
mod tests;
