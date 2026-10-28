//! Physical paths, stages and handles for the coding-agent catalog.

use std::fs::File;
pub(crate) use std::fs::TryLockError;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

pub(crate) fn canonical_catalog_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn catalog_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn remove_catalog_temp(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn link_catalog_temp(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::hard_link(source, destination)
}

pub(crate) fn open_catalog_lock(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

pub(crate) fn catalog_lock_is_regular(file: &File) -> bool {
    file.metadata().map(|meta| meta.is_file()).unwrap_or(false)
}

pub(crate) fn try_catalog_lock(file: &File) -> Result<(), TryLockError> {
    file.try_lock()
}

pub(crate) fn sync_parent_directory(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub(crate) fn write_synced_temp(temp: &Path, bytes: &[u8]) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp)
            .map_err(|e| format!("create temp {} ({e})", temp.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("write temp {} ({e})", temp.display()))?;
        file.flush()
            .map_err(|e| format!("flush temp {} ({e})", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync temp {} ({e})", temp.display()))?;
        drop(file);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

pub(crate) fn tree_byte_count(dir: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else if meta.is_file() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0_u64;
    walk(dir, &mut total);
    total
}
